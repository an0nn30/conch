//! SSH connection dialog — full-featured, matching the Java variant.

use conch_core::models::ServerEntry;

use crate::ui::widgets::{dialog_button, dialog_button_enabled};
const LABEL_WIDTH: f32 = 120.0;

/// Proxy routing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyType {
    None,
    ProxyJump,
    ProxyCommand,
}

impl ProxyType {
    const ALL: [ProxyType; 3] = [ProxyType::None, ProxyType::ProxyJump, ProxyType::ProxyCommand];

    fn label(self) -> &'static str {
        match self {
            ProxyType::None => "None",
            ProxyType::ProxyJump => "ProxyJump",
            ProxyType::ProxyCommand => "ProxyCommand",
        }
    }
}

/// State for the new connection dialog form.
#[derive(Debug, Clone)]
pub struct NewConnectionForm {
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub identity_file: String,
    pub startup_command: String,
    pub show_advanced: bool,
    pub proxy_type: ProxyType,
    pub proxy_value: String,
    pub folder_index: usize,
    /// Auto-focus the host field on first render.
    pub focus_host: bool,
    /// True when editing an existing server (changes dialog title and save behavior).
    pub editing: bool,
}

impl Default for NewConnectionForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: "22".into(),
            user: String::new(),
            password: String::new(),
            identity_file: String::new(),
            startup_command: String::new(),
            show_advanced: false,
            proxy_type: ProxyType::None,
            proxy_value: String::new(),
            folder_index: 0,
            focus_host: true,
            editing: false,
        }
    }
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

    /// Create a form pre-filled from an existing server entry (for editing).
    pub fn from_server_entry(entry: &ServerEntry, folder_index: usize) -> Self {
        let (proxy_type, proxy_value) = if let Some(ref pj) = entry.proxy_jump {
            (ProxyType::ProxyJump, pj.clone())
        } else if let Some(ref pc) = entry.proxy_command {
            (ProxyType::ProxyCommand, pc.clone())
        } else {
            (ProxyType::None, String::new())
        };

        let has_advanced = proxy_type != ProxyType::None;

        Self {
            name: entry.name.clone(),
            host: entry.host.clone(),
            port: entry.port.to_string(),
            user: entry.user.clone(),
            password: String::new(),
            identity_file: entry.identity_file.clone().unwrap_or_default(),
            startup_command: entry.startup_command.clone().unwrap_or_default(),
            show_advanced: has_advanced,
            proxy_type,
            proxy_value,
            folder_index,
            focus_host: false,
            editing: true,
        }
    }

    pub fn port_value(&self) -> u16 {
        self.port.parse().unwrap_or(22)
    }

    /// Build a `ServerEntry` from the current form state.
    pub fn to_server_entry(&self) -> ServerEntry {
        let name = if self.name.trim().is_empty() {
            format!("{}@{}", self.user, self.host)
        } else {
            self.name.clone()
        };

        let identity_file = if self.identity_file.is_empty() {
            None
        } else {
            Some(self.identity_file.clone())
        };

        let startup_command = if self.startup_command.is_empty() {
            None
        } else {
            Some(self.startup_command.clone())
        };

        let (proxy_command, proxy_jump) = match self.proxy_type {
            ProxyType::None => (None, None),
            ProxyType::ProxyJump => {
                let v = self.proxy_value.trim();
                if v.is_empty() { (None, None) } else { (None, Some(v.to_string())) }
            }
            ProxyType::ProxyCommand => {
                let v = self.proxy_value.trim();
                if v.is_empty() { (None, None) } else { (Some(v.to_string()), None) }
            }
        };

        ServerEntry {
            name,
            host: self.host.clone(),
            port: self.port_value(),
            user: self.user.clone(),
            identity_file,
            proxy_command,
            proxy_jump,
            startup_command,
            session_key: None,
            from_ssh_config: false,
        }
    }

    /// Return the password as `Option<String>` (None when empty).
    fn password_opt(&self) -> Option<String> {
        if self.password.is_empty() {
            None
        } else {
            Some(self.password.clone())
        }
    }
}

/// Action returned by the connection dialog each frame.
pub enum DialogAction {
    /// No interaction this frame.
    None,
    /// User clicked Save — persist entry but don't connect.
    Save {
        entry: ServerEntry,
        folder_index: usize,
    },
    /// User clicked Save & Connect — persist entry and start SSH.
    SaveAndConnect {
        entry: ServerEntry,
        folder_index: usize,
        password: Option<String>,
    },
    /// User cancelled the dialog.
    Cancel,
}

/// Show the new connection dialog. Returns the action taken.
pub fn show_new_connection(
    ctx: &egui::Context,
    form: &mut NewConnectionForm,
    folder_names: &[String],
) -> DialogAction {
    let mut action = DialogAction::None;
    let title = if form.editing { "Edit SSH Connection" } else { "New SSH Connection" };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_size([460.0, 280.0])
        .show(ctx, |ui| {
            egui::Grid::new("new_conn_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .min_col_width(LABEL_WIDTH)
                .show(ui, |ui| {
                    // Session Name
                    ui.label("Session Name:");
                    ui.add(
                        crate::ui::widgets::text_edit(&mut form.name)
                            .hint_text("optional")
                            .desired_width(ui.available_width()),
                    );
                    ui.end_row();

                    // Host / Port
                    ui.label("Host / IP:");
                    ui.horizontal(|ui| {
                        let host_resp = ui.add(
                            crate::ui::widgets::text_edit(&mut form.host)
                                .desired_width(ui.available_width() - 80.0),
                        );
                        if form.focus_host {
                            host_resp.request_focus();
                            form.focus_host = false;
                        }
                        ui.label(":");
                        ui.add(
                            crate::ui::widgets::text_edit(&mut form.port)
                                .desired_width(50.0),
                        );
                    });
                    ui.end_row();

                    // Username
                    ui.label("Username:");
                    ui.add(
                        crate::ui::widgets::text_edit(&mut form.user)
                            .desired_width(ui.available_width()),
                    );
                    ui.end_row();

                    // Password
                    ui.label("Password:");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.password)
                            .password(true)
                            .desired_width(ui.available_width()),
                    );
                    ui.end_row();

                    // Private Key + Browse
                    ui.label("Private Key:");
                    ui.horizontal(|ui| {
                        ui.add(
                            crate::ui::widgets::text_edit(&mut form.identity_file)
                                .desired_width(ui.available_width() - 80.0),
                        );
                        if ui.button("Browse\u{2026}").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_directory(dirs::home_dir().unwrap_or_default().join(".ssh"))
                                .pick_file()
                            {
                                form.identity_file = path.display().to_string();
                            }
                        }
                    });
                    ui.end_row();

                    // Startup Command
                    ui.label("Startup Command:");
                    ui.add(
                        crate::ui::widgets::text_edit(&mut form.startup_command)
                            .hint_text("optional")
                            .desired_width(ui.available_width()),
                    );
                    ui.end_row();
                });

            ui.add_space(4.0);

            // Collapsible Advanced section
            let advanced_id = ui.make_persistent_id("new_conn_advanced");
            let header_text = if form.show_advanced {
                "\u{25BC} Advanced"
            } else {
                "\u{25B6} Advanced"
            };
            if ui
                .add(egui::Label::new(header_text).sense(egui::Sense::click()))
                .clicked()
            {
                form.show_advanced = !form.show_advanced;
            }

            if form.show_advanced {
                ui.indent(advanced_id, |ui| {
                    egui::Grid::new("new_conn_adv_grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .min_col_width(LABEL_WIDTH)
                        .show(ui, |ui| {
                            // Proxy Type dropdown
                            ui.label("Proxy Type:");
                            egui::ComboBox::from_id_salt("proxy_type")
                                .selected_text(form.proxy_type.label())
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    for pt in ProxyType::ALL {
                                        ui.selectable_value(
                                            &mut form.proxy_type,
                                            pt,
                                            pt.label(),
                                        );
                                    }
                                });
                            ui.end_row();

                            // Conditional proxy field
                            match form.proxy_type {
                                ProxyType::ProxyJump => {
                                    ui.label("Jump Host:");
                                    ui.add(
                                        crate::ui::widgets::text_edit(&mut form.proxy_value)
                                            .hint_text("user@jumphost:port")
                                            .desired_width(ui.available_width()),
                                    );
                                    ui.end_row();
                                }
                                ProxyType::ProxyCommand => {
                                    ui.label("Proxy Command:");
                                    ui.add(
                                        crate::ui::widgets::text_edit(&mut form.proxy_value)
                                            .hint_text("ssh -W %h:%p jumphost")
                                            .desired_width(ui.available_width()),
                                    );
                                    ui.end_row();
                                }
                                ProxyType::None => {}
                            }
                        });
                });
            }

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            // Folder dropdown
            if !folder_names.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Save to folder:");
                    egui::ComboBox::from_id_salt("folder_select")
                        .selected_text(&folder_names[form.folder_index.min(folder_names.len().saturating_sub(1))])
                        .show_ui(ui, |ui| {
                            for (i, name) in folder_names.iter().enumerate() {
                                ui.selectable_value(&mut form.folder_index, i, name);
                            }
                        });
                });
                ui.add_space(4.0);
            }

            // Button row — right-aligned: Cancel | Save | Save & Connect
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_connect = !form.host.trim().is_empty();

                    if dialog_button_enabled(ui, "Save & Connect", can_connect).clicked() {
                        action = DialogAction::SaveAndConnect {
                            entry: form.to_server_entry(),
                            folder_index: form.folder_index,
                            password: form.password_opt(),
                        };
                    }

                    if dialog_button_enabled(ui, "Save", can_connect).clicked() {
                        action = DialogAction::Save {
                            entry: form.to_server_entry(),
                            folder_index: form.folder_index,
                        };
                    }

                    if dialog_button(ui, "Cancel").clicked() {
                        action = DialogAction::Cancel;
                    }
                });
            });
        });

    action
}
