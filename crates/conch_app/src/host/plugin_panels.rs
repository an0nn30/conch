//! Plugin panel rendering with tabbed multi-panel support.

use std::collections::HashMap;

use conch_plugin::bus::PluginMail;
use conch_plugin_sdk::widgets::{PluginEvent, Widget};
use conch_plugin_sdk::PanelLocation;

use crate::app::ConchApp;
use crate::ui_theme::UiTheme;

/// Seed egui's internal panel state with a persisted width.
/// Uses the available rect to construct a properly positioned panel rect.
/// Only writes if no state exists yet (i.e., first frame this panel appears).
fn seed_side_panel(ctx: &egui::Context, id: egui::Id, width: f32, is_right: bool) {
    use egui::containers::panel::PanelState;
    if PanelState::load(ctx, id).is_some() {
        return;
    }
    let available = ctx.available_rect();
    let rect = if is_right {
        egui::Rect::from_min_max(
            egui::pos2(available.max.x - width, available.min.y),
            available.max,
        )
    } else {
        egui::Rect::from_min_max(
            available.min,
            egui::pos2(available.min.x + width, available.max.y),
        )
    };
    ctx.data_mut(|d| d.insert_persisted(id, PanelState { rect }));
}

/// Seed egui's internal panel state with a persisted height (bottom panels).
fn seed_bottom_panel(ctx: &egui::Context, id: egui::Id, height: f32) {
    use egui::containers::panel::PanelState;
    if PanelState::load(ctx, id).is_some() {
        return;
    }
    let available = ctx.available_rect();
    let rect = egui::Rect::from_min_max(
        egui::pos2(available.min.x, available.max.y - height),
        available.max,
    );
    ctx.data_mut(|d| d.insert_persisted(id, PanelState { rect }));
}

const DEFAULT_SIDE_WIDTH: f32 = 240.0;
const DEFAULT_BOTTOM_HEIGHT: f32 = 180.0;

/// Width of the vertical tab strip panel.
const TAB_STRIP_WIDTH: f32 = 28.0;
/// Width of the accent bar on the active tab.
const ACCENT_WIDTH: f32 = 3.0;

impl ConchApp {
    /// Render plugin panels into egui side/bottom panels with tabbing.
    ///
    /// When multiple plugins register at the same location, they share a single
    /// egui panel with a vertical tab strip on the outer edge.
    pub(crate) fn render_plugin_panels(&mut self, ctx: &egui::Context) {
        let theme = self.state.theme.clone();

        // Group panels by location, sorted by handle for stable order.
        let mut by_location: HashMap<PanelLocation, Vec<(u64, String, String)>> = HashMap::new();
        {
            let reg = self.panel_registry.lock();
            for (handle, info) in reg.panels() {
                if info.location == PanelLocation::None {
                    continue;
                }
                by_location
                    .entry(info.location)
                    .or_default()
                    .push((handle, info.plugin_name.clone(), info.name.clone()));
            }
        }
        for group in by_location.values_mut() {
            group.sort_by_key(|(h, _, _)| *h);
        }

        // Collect events to dispatch after rendering (avoids borrow issues).
        let mut all_events: Vec<(String, Vec<conch_plugin_sdk::widgets::WidgetEvent>)> = Vec::new();

        // Render each location in a fixed order.
        for location in [PanelLocation::Left, PanelLocation::Right, PanelLocation::Bottom] {
            let Some(panels) = by_location.get(&location) else {
                continue;
            };

            // Check visibility toggle.
            match location {
                PanelLocation::Left if !self.left_panel_visible => continue,
                PanelLocation::Right if !self.right_panel_visible => continue,
                PanelLocation::Bottom if !self.bottom_panel_visible => continue,
                _ => {}
            }

            // Validate/default the active tab for this location.
            let active_handle = {
                let entry = self.active_panel_tab.entry(location).or_insert(panels[0].0);
                if !panels.iter().any(|(h, _, _)| *h == *entry) {
                    *entry = panels[0].0;
                }
                *entry
            };

            // Find the active panel's plugin name and display name.
            let (_, active_plugin, active_name) = panels
                .iter()
                .find(|(h, _, _)| *h == active_handle)
                .unwrap();
            let active_plugin = active_plugin.clone();
            let active_name = active_name.clone();

            // Get cached widget JSON for the active plugin.
            let json = self
                .render_cache
                .get(&active_plugin)
                .cloned()
                .unwrap_or_else(|| "[]".to_string());
            let widgets: Vec<Widget> = serde_json::from_str(&json).unwrap_or_default();

            let multi = panels.len() > 1;
            let tab_data: Vec<(u64, String)> =
                panels.iter().map(|(h, _, name)| (*h, name.clone())).collect();
            let panel_id = format!("plugin_loc_{location:?}");

            let mut widget_events = Vec::new();
            let mut new_active: Option<u64> = None;

            // Use persisted sizes when available, otherwise fall back to defaults.
            let layout = &self.state.persistent.layout;
            let left_w = if layout.left_panel_width > 0.0 { layout.left_panel_width } else { DEFAULT_SIDE_WIDTH };
            let right_w = if layout.right_panel_width > 0.0 { layout.right_panel_width } else { DEFAULT_SIDE_WIDTH };
            let bottom_h = if layout.bottom_panel_height > 0.0 { layout.bottom_panel_height } else { DEFAULT_BOTTOM_HEIGHT };

            match location {
                PanelLocation::Left => {
                    // Tab strip as a separate narrow panel (outermost edge).
                    if multi {
                        let strip_id = format!("{panel_id}_tabs");
                        new_active = show_tab_strip_panel(
                            ctx,
                            &strip_id,
                            &tab_data,
                            active_handle,
                            &theme,
                            location,
                        );
                    }
                    // Seed persisted width into egui memory on first appearance.
                    let eid = egui::Id::new(&panel_id);
                    if left_w > 0.0 {
                        seed_side_panel(ctx, eid, left_w, false);
                    }
                    // Content panel.
                    let resp = egui::SidePanel::left(eid)
                        .default_width(left_w)
                        .resizable(true)
                        .frame(egui::Frame::NONE.fill(theme.surface).inner_margin(8.0))
                        .show(ctx, |ui| {
                            ui.set_min_width(ui.available_width());
                            if !multi {
                                ui.label(
                                    egui::RichText::new(&active_name)
                                        .size(theme.font_normal + 1.0)
                                        .strong()
                                        .color(theme.text),
                                );
                                ui.separator();
                            }
                            widget_events =
                                crate::host::panel_renderer::render_widgets(
                                    ui,
                                    &widgets,
                                    &theme,
                                    &mut self.plugin_text_state,
                                );
                        });
                    self.state.persistent.layout.left_panel_width = resp.response.rect.width();
                }
                PanelLocation::Right => {
                    // Tab strip as a separate narrow panel (outermost edge).
                    if multi {
                        let strip_id = format!("{panel_id}_tabs");
                        new_active = show_tab_strip_panel(
                            ctx,
                            &strip_id,
                            &tab_data,
                            active_handle,
                            &theme,
                            location,
                        );
                    }
                    // Seed persisted width into egui memory on first appearance.
                    let eid = egui::Id::new(&panel_id);
                    if right_w > 0.0 {
                        seed_side_panel(ctx, eid, right_w, true);
                    }
                    // Content panel.
                    let resp = egui::SidePanel::right(eid)
                        .default_width(right_w)
                        .resizable(true)
                        .frame(egui::Frame::NONE.fill(theme.surface).inner_margin(8.0))
                        .show(ctx, |ui| {
                            ui.set_min_width(ui.available_width());
                            if !multi {
                                ui.label(
                                    egui::RichText::new(&active_name)
                                        .size(theme.font_normal + 1.0)
                                        .strong()
                                        .color(theme.text),
                                );
                                ui.separator();
                            }
                            widget_events =
                                crate::host::panel_renderer::render_widgets(
                                    ui,
                                    &widgets,
                                    &theme,
                                    &mut self.plugin_text_state,
                                );
                        });
                    self.state.persistent.layout.right_panel_width = resp.response.rect.width();
                }
                PanelLocation::Bottom => {
                    // Seed persisted height into egui memory on first appearance.
                    let eid = egui::Id::new(&panel_id);
                    if bottom_h > 0.0 {
                        seed_bottom_panel(ctx, eid, bottom_h);
                    }
                    let resp = egui::TopBottomPanel::bottom(eid)
                        .default_height(bottom_h)
                        .resizable(true)
                        .frame(egui::Frame::NONE.fill(theme.surface).inner_margin(8.0))
                        .show(ctx, |ui| {
                            ui.set_min_height(ui.available_height());
                            if multi {
                                ui.horizontal(|ui| {
                                    for (handle, name) in &tab_data {
                                        let is_active = *handle == active_handle;
                                        let text = egui::RichText::new(name)
                                            .size(theme.font_small)
                                            .color(if is_active {
                                                theme.accent
                                            } else {
                                                theme.text_secondary
                                            });
                                        if ui.selectable_label(is_active, text).clicked() {
                                            new_active = Some(*handle);
                                        }
                                    }
                                });
                                ui.separator();
                            } else {
                                ui.label(
                                    egui::RichText::new(&active_name)
                                        .size(theme.font_normal + 1.0)
                                        .strong()
                                        .color(theme.text),
                                );
                                ui.separator();
                            }
                            widget_events = crate::host::panel_renderer::render_widgets(
                                ui,
                                &widgets,
                                &theme,
                                &mut self.plugin_text_state,
                            );
                        });
                    self.state.persistent.layout.bottom_panel_height = resp.response.rect.height();
                }
                _ => {}
            }

            // Update active tab if a tab was clicked.
            if let Some(h) = new_active {
                self.active_panel_tab.insert(location, h);
            }

            // Collect events for dispatch after the loop.
            if !widget_events.is_empty() {
                all_events.push((active_plugin, widget_events));
            }
        }

        // Dispatch widget events back to plugins.
        for (plugin_name, events) in all_events {
            if let Some(sender) = self.plugin_bus.sender_for(&plugin_name) {
                for event in events {
                    let plugin_event = PluginEvent::Widget(event);
                    if let Ok(json) = serde_json::to_string(&plugin_event) {
                        let _ = sender.try_send(PluginMail::WidgetEvent { json });
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Vertical tab strip for multi-panel locations
// ---------------------------------------------------------------------------

/// Render a vertical tab strip as a separate narrow `SidePanel`.
///
/// Matches the style of the built-in Files/Plugins sidebar tabs.
/// Returns the handle of a newly clicked tab, if any.
fn show_tab_strip_panel(
    ctx: &egui::Context,
    panel_id: &str,
    tabs: &[(u64, String)],
    active_handle: u64,
    theme: &UiTheme,
    side: PanelLocation,
) -> Option<u64> {
    let mut clicked = None;

    let panel = match side {
        PanelLocation::Left => egui::SidePanel::left(egui::Id::new(panel_id)),
        PanelLocation::Right => egui::SidePanel::right(egui::Id::new(panel_id)),
        _ => return None,
    };

    let darker_bg = darken_color(theme.surface, 18);

    panel
        .resizable(false)
        .exact_width(TAB_STRIP_WIDTH)
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let panel_rect = ui.available_rect_before_wrap();
            let painter = ui.painter_at(panel_rect);

            let font_id = egui::FontId::new(13.0, egui::FontFamily::Proportional);
            let tab_height = panel_rect.height() / tabs.len() as f32;

            // Fill the entire strip with a darker background.
            painter.rect_filled(panel_rect, 0.0, darker_bg);

            for (i, (handle, name)) in tabs.iter().enumerate() {
                let y_min = panel_rect.min.y + i as f32 * tab_height;
                let tab_rect = egui::Rect::from_min_size(
                    egui::pos2(panel_rect.min.x, y_min),
                    egui::vec2(TAB_STRIP_WIDTH, tab_height),
                );

                let selected = *handle == active_handle;

                // Selected tab gets the base surface color.
                if selected {
                    painter.rect_filled(tab_rect, 0.0, theme.surface);

                    // Accent bar on the inner edge.
                    let accent_rect = match side {
                        PanelLocation::Left => egui::Rect::from_min_size(
                            egui::pos2(tab_rect.max.x - ACCENT_WIDTH, tab_rect.min.y),
                            egui::vec2(ACCENT_WIDTH, tab_height),
                        ),
                        _ => egui::Rect::from_min_size(
                            egui::pos2(tab_rect.min.x, tab_rect.min.y),
                            egui::vec2(ACCENT_WIDTH, tab_height),
                        ),
                    };
                    painter.rect_filled(accent_rect, 0.0, theme.accent);
                }

                // Rotated text: -90° so it reads bottom-to-top.
                let text_color = if selected { theme.accent } else { theme.text_secondary };
                let galley = painter.layout_no_wrap(name.clone(), font_id.clone(), text_color);
                let text_w = galley.size().x;
                let text_h = galley.size().y;

                let cx = tab_rect.center().x;
                let cy = tab_rect.center().y;
                let text_top = cy - text_w / 2.0;
                let pos = egui::pos2(cx - text_h / 2.0, text_top + text_w);

                let text_shape =
                    egui::epaint::TextShape::new(pos, std::sync::Arc::clone(&galley), text_color)
                        .with_angle(-std::f32::consts::FRAC_PI_2);
                painter.add(egui::Shape::Text(text_shape));

                // Click interaction.
                let response = ui.interact(tab_rect, ui.id().with(*handle), egui::Sense::click());
                if response.clicked() {
                    clicked = Some(*handle);
                }
                response.on_hover_text(name);
            }
        });

    clicked
}

/// Darken a color by subtracting from each channel.
fn darken_color(c: egui::Color32, amount: u8) -> egui::Color32 {
    egui::Color32::from_rgb(
        c.r().saturating_sub(amount),
        c.g().saturating_sub(amount),
        c.b().saturating_sub(amount),
    )
}
