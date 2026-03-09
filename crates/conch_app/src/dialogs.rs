//! Dialog management: open/close tracking and the About dialog.

use crate::app::ConchApp;
use crate::plugins::send_plugin_dialog_cancel;

impl ConchApp {
    /// Returns true when any modal dialog is open and should steal focus from the terminal.
    pub(crate) fn any_dialog_open(&self) -> bool {
        self.state.new_connection_form.is_some()
            || self.show_about
            || self.rename_tab_id.is_some()
            || self.preferences_form.is_some()
            || self.tunnel_dialog.is_some()
            || self.active_plugin_dialog.is_some()
            || self.plugin_progress.is_some()
            || self.notification_history_dialog.is_some()
    }

    /// Close the topmost dialog. Returns true if a dialog was closed.
    pub(crate) fn close_topmost_dialog(&mut self) -> bool {
        if let Some(dialog) = self.active_plugin_dialog.take() {
            send_plugin_dialog_cancel(&dialog);
            return true;
        }
        if self.plugin_progress.is_some() {
            self.plugin_progress = None;
            return true;
        }
        if self.notification_history_dialog.is_some() {
            self.notification_history_dialog = None;
            return true;
        }
        if self.show_about {
            self.show_about = false;
            return true;
        }
        if self.rename_tab_id.is_some() {
            self.rename_tab_id = None;
            self.rename_tab_buf.clear();
            return true;
        }
        if self.preferences_form.is_some() {
            self.preferences_form = None;
            return true;
        }
        if self.tunnel_dialog.is_some() {
            self.tunnel_dialog = None;
            return true;
        }
        if self.state.new_connection_form.is_some() {
            self.state.new_connection_form = None;
            return true;
        }
        false
    }

    /// Show the About Conch dialog.
    pub(crate) fn show_about_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("About Conch")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Conch");
                    ui.label("Version 0.2");
                    ui.add_space(4.0);
                    ui.label("A cross-platform SSH terminal emulator.");
                    ui.add_space(8.0);
                    if crate::ui::widgets::dialog_button(ui, "OK").clicked() {
                        self.show_about = false;
                    }
                });
            });
    }
}
