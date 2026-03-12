//! Fallback in-window menu bar using egui widgets.
//! Used on Linux and Windows where there's no native global menu.

use super::MenuAction;

/// Render the egui in-window menu bar. Returns any triggered action.
pub fn show(ctx: &egui::Context) -> Option<MenuAction> {
    let mut action = None;
    let menu_width = ctx.style().spacing.menu_width;

    egui::TopBottomPanel::top("menu_bar")
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
                });

                ui.menu_button("Help", |ui| {
                    ui.set_min_width(menu_width);
                    if ui.button("About Conch").clicked() {
                        // TODO: show about dialog
                        ui.close_menu();
                    }
                });
            });
        });

    action
}
