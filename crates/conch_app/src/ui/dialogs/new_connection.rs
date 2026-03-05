//! SSH connection dialog.

const BTN_MIN_SIZE: egui::Vec2 = egui::Vec2::new(95.0, 26.0);

/// State for the new connection dialog form.
#[derive(Debug, Clone, Default)]
pub struct NewConnectionForm {
    pub host: String,
    pub port: String,
    pub user: String,
    pub identity_file: String,
}

impl NewConnectionForm {
    /// Create a form pre-filled with sensible defaults (port 22, current user).
    pub fn with_defaults() -> Self {
        Self {
            port: "22".into(),
            user: std::env::var("USER").unwrap_or_default(),
            ..Default::default()
        }
    }

    pub fn port_value(&self) -> u16 {
        self.port.parse().unwrap_or(22)
    }
}

/// Action returned by the connection dialog each frame.
pub enum DialogAction {
    /// No interaction this frame.
    None,
    /// User clicked Connect.
    Connect {
        host: String,
        port: u16,
        user: String,
        identity_file: Option<String>,
    },
    /// User cancelled the dialog.
    Cancel,
}

/// Show the new connection dialog as an `egui::Window`. Returns the action taken.
pub fn show_new_connection(ctx: &egui::Context, form: &mut NewConnectionForm) -> DialogAction {
    let mut action = DialogAction::None;

    egui::Window::new("New SSH Connection")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_size([400.0, 200.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Host:");
                ui.add(crate::ui::widgets::text_edit(&mut form.host));
                ui.label(":");
                ui.add(crate::ui::widgets::text_edit(&mut form.port).desired_width(50.0));
            });
            ui.horizontal(|ui| {
                ui.label("User:");
                ui.add(crate::ui::widgets::text_edit(&mut form.user));
            });
            ui.horizontal(|ui| {
                ui.label("Identity File:");
                ui.add(crate::ui::widgets::text_edit(&mut form.identity_file));
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_sized(BTN_MIN_SIZE, egui::Button::new(egui::RichText::new("Connect").size(14.0))).clicked() {
                        let identity = if form.identity_file.is_empty() {
                            None
                        } else {
                            Some(form.identity_file.clone())
                        };
                        action = DialogAction::Connect {
                            host: form.host.clone(),
                            port: form.port_value(),
                            user: form.user.clone(),
                            identity_file: identity,
                        };
                    }
                    if ui.add_sized(BTN_MIN_SIZE, egui::Button::new(egui::RichText::new("Cancel").size(14.0))).clicked() {
                        action = DialogAction::Cancel;
                    }
                });
            });
        });

    action
}
