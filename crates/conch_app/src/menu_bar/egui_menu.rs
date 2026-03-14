//! Fallback in-window menu bar using egui widgets.
//! Used on Linux and Windows where there's no native global menu.

use crate::host::bridge;
use super::MenuAction;

/// Render the egui in-window menu bar. Returns any triggered action.
///
/// `panel_id` must be unique per viewport to prevent menu state leaking
/// between windows (e.g. opening a menu in one window opens it in all).
pub fn show_with_id(ctx: &egui::Context, panel_id: egui::Id) -> Option<MenuAction> {
    let mut action = None;
    let menu_width = ctx.style().spacing.menu_width;

    // Group plugin items by menu name for merging into standard menus.
    let plugin_items = bridge::plugin_menu_items();
    let mut plugin_menus = std::collections::BTreeMap::<String, Vec<&bridge::PluginMenuItem>>::new();
    for item in &plugin_items {
        plugin_menus
            .entry(item.menu.clone())
            .or_default()
            .push(item);
    }

    let standard_menus = ["File", "Edit", "View", "Help"];

    egui::TopBottomPanel::top(panel_id)
        .frame(egui::Frame::NONE.fill(ctx.style().visuals.panel_fill))
        .show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    ui.set_min_width(menu_width);
                    if ui.button("New Tab").clicked() {
                        action = Some(MenuAction::NewTab);
                        ui.close_menu();
                    }
                    if ui.button("New Window").clicked() {
                        action = Some(MenuAction::NewWindow);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Close Tab").clicked() {
                        action = Some(MenuAction::CloseTab);
                        ui.close_menu();
                    }
                    // Append plugin items registered under "File".
                    render_plugin_items(ui, plugin_menus.get("File"), &mut action);
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        action = Some(MenuAction::Quit);
                        ui.close_menu();
                    }
                });

                ui.menu_button("Edit", |ui| {
                    ui.set_min_width(menu_width);
                    if ui.button("Copy").clicked() {
                        action = Some(MenuAction::Copy);
                        ui.close_menu();
                    }
                    if ui.button("Paste").clicked() {
                        action = Some(MenuAction::Paste);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Select All").clicked() {
                        action = Some(MenuAction::SelectAll);
                        ui.close_menu();
                    }
                    // Append plugin items registered under "Edit".
                    render_plugin_items(ui, plugin_menus.get("Edit"), &mut action);
                });

                ui.menu_button("View", |ui| {
                    ui.set_min_width(menu_width);
                    if ui.button("Plugin Manager").clicked() {
                        action = Some(MenuAction::PluginManager);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Zen Mode").clicked() {
                        action = Some(MenuAction::ZenMode);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Zoom In").clicked() {
                        action = Some(MenuAction::ZoomIn);
                        ui.close_menu();
                    }
                    if ui.button("Zoom Out").clicked() {
                        action = Some(MenuAction::ZoomOut);
                        ui.close_menu();
                    }
                    if ui.button("Reset Zoom").clicked() {
                        action = Some(MenuAction::ZoomReset);
                        ui.close_menu();
                    }
                    // Append plugin items registered under "View".
                    render_plugin_items(ui, plugin_menus.get("View"), &mut action);
                });

                // Custom plugin menus (not File/Edit/View/Help) get their own buttons.
                for (menu_name, items) in &plugin_menus {
                    if standard_menus.contains(&menu_name.as_str())
                        || menu_name.starts_with('_')
                    {
                        continue;
                    }
                    ui.menu_button(menu_name.as_str(), |ui| {
                        ui.set_min_width(menu_width);
                        for item in items {
                            if ui.button(&item.label).clicked() {
                                action = Some(MenuAction::PluginAction {
                                    plugin_name: item.plugin_name.clone(),
                                    action: item.action.clone(),
                                });
                                ui.close_menu();
                            }
                        }
                    });
                }

                ui.menu_button("Help", |ui| {
                    ui.set_min_width(menu_width);
                    if ui.button("About Conch").clicked() {
                        // TODO: show about dialog
                        ui.close_menu();
                    }
                    // Append plugin items registered under "Help".
                    render_plugin_items(ui, plugin_menus.get("Help"), &mut action);
                });
            });
        });

    action
}

/// Render plugin-registered items inside a standard menu, preceded by a separator.
fn render_plugin_items(
    ui: &mut egui::Ui,
    items: Option<&Vec<&bridge::PluginMenuItem>>,
    action: &mut Option<MenuAction>,
) {
    if let Some(items) = items {
        if !items.is_empty() {
            ui.separator();
            for item in items {
                if ui.button(&item.label).clicked() {
                    *action = Some(MenuAction::PluginAction {
                        plugin_name: item.plugin_name.clone(),
                        action: item.action.clone(),
                    });
                    ui.close_menu();
                }
            }
        }
    }
}

/// Render the egui in-window menu bar for the main window.
pub fn show(ctx: &egui::Context) -> Option<MenuAction> {
    show_with_id(ctx, egui::Id::new("menu_bar"))
}
