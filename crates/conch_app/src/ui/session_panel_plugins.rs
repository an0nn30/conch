//! Plugin tab area for the bottom half of the right sidebar (session panel).

use std::collections::HashMap;

use egui::RichText;
use egui_extras::{Column, TableBuilder};

/// Actions that can be triggered by session panel plugins.
pub enum SessionPanelPluginAction {
    None,
    /// A button was clicked in a session panel plugin.
    PanelButtonClick { plugin_idx: usize, button_id: String },
    /// Deactivate (stop) a session panel plugin.
    DeactivatePanel(usize),
}

/// Render the session panel plugin area with tabs and content.
pub fn show_session_panel_plugins(
    ui: &mut egui::Ui,
    tabs: &[usize],
    active_tab: &mut Option<usize>,
    panel_widgets: &HashMap<usize, Vec<conch_plugin::PanelWidget>>,
    panel_names: &HashMap<usize, String>,
) -> SessionPanelPluginAction {
    let mut action = SessionPanelPluginAction::None;

    // Tab bar at the top.
    let tab_bar_height = 24.0;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), tab_bar_height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            for &idx in tabs {
                let name = panel_names.get(&idx).map(|s| s.as_str()).unwrap_or("Panel");
                let is_active = *active_tab == Some(idx);

                let text = if is_active {
                    RichText::new(name).size(11.0).strong()
                } else {
                    RichText::new(name).size(11.0)
                };

                let resp = ui.selectable_label(is_active, text);
                if resp.clicked() {
                    *active_tab = Some(idx);
                }

                resp.context_menu(|ui| {
                    if ui.button("Close").clicked() {
                        action = SessionPanelPluginAction::DeactivatePanel(idx);
                        ui.close_menu();
                    }
                });
            }
        },
    );

    ui.separator();

    // Render the active tab's content.
    if let Some(idx) = *active_tab {
        let widgets = panel_widgets.get(&idx).map(|v| v.as_slice()).unwrap_or(&[]);
        let inner = render_panel_widgets(ui, idx, widgets);
        if !matches!(inner, SessionPanelPluginAction::None) {
            action = inner;
        }
    }

    action
}

/// Render a panel plugin's declarative widgets.
fn render_panel_widgets(
    ui: &mut egui::Ui,
    plugin_idx: usize,
    widgets: &[conch_plugin::PanelWidget],
) -> SessionPanelPluginAction {
    let mut action = SessionPanelPluginAction::None;

    if widgets.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.weak("Loading...");
        });
        return action;
    }

    let has_scroll_text = widgets
        .iter()
        .any(|w| matches!(w, conch_plugin::PanelWidget::ScrollText(_)));

    egui::ScrollArea::vertical()
        .id_salt(("session_panel_plugin", plugin_idx))
        .stick_to_bottom(has_scroll_text)
        .show(ui, |ui| {
            for widget in widgets {
                match widget {
                    conch_plugin::PanelWidget::ScrollText(lines) => {
                        for line in lines {
                            ui.label(RichText::new(line).monospace().size(11.0));
                        }
                    }
                    conch_plugin::PanelWidget::Heading(text) => {
                        ui.add_space(4.0);
                        ui.strong(text);
                        ui.add_space(2.0);
                    }
                    conch_plugin::PanelWidget::Text(text) => {
                        ui.label(RichText::new(text).monospace().size(11.0));
                    }
                    conch_plugin::PanelWidget::Label(text) => {
                        ui.label(text);
                    }
                    conch_plugin::PanelWidget::Separator => {
                        ui.separator();
                    }
                    conch_plugin::PanelWidget::Table { columns, rows } => {
                        let num_cols = columns.len();
                        if num_cols > 0 {
                            TableBuilder::new(ui)
                                .striped(true)
                                .resizable(true)
                                .columns(Column::remainder().at_least(40.0), num_cols)
                                .header(16.0, |mut header| {
                                    for col in columns {
                                        header.col(|ui| {
                                            ui.label(RichText::new(col).strong().size(10.0));
                                        });
                                    }
                                })
                                .body(|body| {
                                    body.rows(16.0, rows.len(), |mut row| {
                                        let idx = row.index();
                                        if let Some(cells) = rows.get(idx) {
                                            for cell in cells {
                                                row.col(|ui| {
                                                    ui.label(
                                                        RichText::new(cell).size(11.0).monospace(),
                                                    );
                                                });
                                            }
                                        }
                                    });
                                });
                        }
                    }
                    conch_plugin::PanelWidget::Progress { label, fraction, text } => {
                        ui.label(RichText::new(label).size(11.0));
                        let bar = egui::ProgressBar::new(*fraction)
                            .text(text)
                            .desired_width(ui.available_width());
                        ui.add(bar);
                    }
                    conch_plugin::PanelWidget::Button { id, label } => {
                        if ui.button(label).clicked() {
                            action = SessionPanelPluginAction::PanelButtonClick {
                                plugin_idx,
                                button_id: id.clone(),
                            };
                        }
                    }
                    conch_plugin::PanelWidget::KeyValue { key, value } => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(key).strong().size(11.0));
                            ui.label(RichText::new(value).size(11.0).monospace());
                        });
                    }
                }
            }
        });

    action
}
