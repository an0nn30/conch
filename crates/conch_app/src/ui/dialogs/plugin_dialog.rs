//! Plugin-driven dialogs: form, prompt, confirm, alert, error, text viewer, table viewer.
//!
//! Each dialog variant stores its own UI state and a `resp_tx` channel to send
//! the result back to the plugin coroutine when the user submits or cancels.

use std::collections::HashMap;
use std::time::Instant;

use conch_plugin::{FormField, PluginResponse};
use tokio::sync::mpsc;

const BTN_MIN_SIZE: egui::Vec2 = egui::Vec2::new(95.0, 26.0);

/// State for a single form field (mutable values the user edits).
pub enum FormFieldState {
    Text { name: String, label: String, value: String },
    Password { name: String, label: String, value: String },
    ComboBox { name: String, label: String, options: Vec<String>, selected: usize },
    CheckBox { name: String, label: String, checked: bool },
    Separator,
    Label { text: String },
}

impl FormFieldState {
    pub fn from_field(field: &FormField) -> Self {
        match field {
            FormField::Text { name, label, default } => Self::Text {
                name: name.clone(),
                label: label.clone(),
                value: default.clone(),
            },
            FormField::Password { name, label } => Self::Password {
                name: name.clone(),
                label: label.clone(),
                value: String::new(),
            },
            FormField::ComboBox { name, label, options, default } => {
                let selected = options
                    .iter()
                    .position(|o| o == default)
                    .unwrap_or(0);
                Self::ComboBox {
                    name: name.clone(),
                    label: label.clone(),
                    options: options.clone(),
                    selected,
                }
            }
            FormField::CheckBox { name, label, default } => Self::CheckBox {
                name: name.clone(),
                label: label.clone(),
                checked: *default,
            },
            FormField::Separator => Self::Separator,
            FormField::Label { text } => Self::Label { text: text.clone() },
        }
    }

    /// Collect the value from this field, if it has a name.
    fn collect(&self) -> Option<(String, String)> {
        match self {
            Self::Text { name, value, .. } => Some((name.clone(), value.clone())),
            Self::Password { name, value, .. } => Some((name.clone(), value.clone())),
            Self::ComboBox { name, options, selected, .. } => {
                let val = options.get(*selected).cloned().unwrap_or_default();
                Some((name.clone(), val))
            }
            Self::CheckBox { name, checked, .. } => {
                Some((name.clone(), checked.to_string()))
            }
            Self::Separator | Self::Label { .. } => None,
        }
    }
}

/// The currently active plugin dialog. Only one is shown at a time.
pub enum ActivePluginDialog {
    Form {
        title: String,
        fields: Vec<FormFieldState>,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
    },
    Prompt {
        message: String,
        input: String,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
    },
    Confirm {
        message: String,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
    },
    Alert {
        title: String,
        message: String,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
    },
    Error {
        title: String,
        message: String,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
    },
    Text {
        title: String,
        text: String,
        copied_at: Option<Instant>,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
    },
    Table {
        title: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        resp_tx: mpsc::UnboundedSender<PluginResponse>,
    },
}

/// Render the active plugin dialog. Returns `true` if the dialog was closed
/// (submitted or cancelled) and should be removed.
pub fn show_plugin_dialog(ctx: &egui::Context, dialog: &mut ActivePluginDialog) -> bool {
    match dialog {
        ActivePluginDialog::Form { title, fields, resp_tx } => {
            show_form(ctx, title, fields, resp_tx)
        }
        ActivePluginDialog::Prompt { message, input, resp_tx } => {
            show_prompt(ctx, message, input, resp_tx)
        }
        ActivePluginDialog::Confirm { message, resp_tx } => {
            show_confirm(ctx, message, resp_tx)
        }
        ActivePluginDialog::Alert { title, message, resp_tx } => {
            show_alert(ctx, title, message, resp_tx, false)
        }
        ActivePluginDialog::Error { title, message, resp_tx } => {
            show_alert(ctx, title, message, resp_tx, true)
        }
        ActivePluginDialog::Text { title, text, copied_at, resp_tx } => {
            show_text_viewer(ctx, title, text, copied_at, resp_tx)
        }
        ActivePluginDialog::Table { title, columns, rows, resp_tx } => {
            show_table_viewer(ctx, title, columns, rows, resp_tx)
        }
    }
}

fn show_form(
    ctx: &egui::Context,
    title: &str,
    fields: &mut [FormFieldState],
    resp_tx: &mpsc::UnboundedSender<PluginResponse>,
) -> bool {
    let mut closed = false;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_size([420.0, 200.0])
        .show(ctx, |ui| {
            egui::Grid::new("plugin_form_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    for field in fields.iter_mut() {
                        match field {
                            FormFieldState::Text { label, value, .. } => {
                                ui.label(label.as_str());
                                ui.add(
                                    crate::ui::widgets::text_edit(value)
                                        .desired_width(250.0),
                                );
                                ui.end_row();
                            }
                            FormFieldState::Password { label, value, .. } => {
                                ui.label(label.as_str());
                                ui.add(
                                    egui::TextEdit::singleline(value)
                                        .password(true)
                                        .desired_width(250.0),
                                );
                                ui.end_row();
                            }
                            FormFieldState::ComboBox {
                                label,
                                options,
                                selected,
                                name,
                                ..
                            } => {
                                ui.label(label.as_str());
                                let current = options
                                    .get(*selected)
                                    .cloned()
                                    .unwrap_or_default();
                                egui::ComboBox::from_id_salt(name.as_str())
                                    .selected_text(current)
                                    .width(250.0)
                                    .show_ui(ui, |ui| {
                                        for (i, opt) in options.iter().enumerate() {
                                            ui.selectable_value(selected, i, opt);
                                        }
                                    });
                                ui.end_row();
                            }
                            FormFieldState::CheckBox { label, checked, .. } => {
                                ui.label("");
                                ui.checkbox(checked, label.as_str());
                                ui.end_row();
                            }
                            FormFieldState::Separator => {
                                ui.separator();
                                ui.separator();
                                ui.end_row();
                            }
                            FormFieldState::Label { text } => {
                                ui.label("");
                                ui.label(
                                    egui::RichText::new(text.as_str()).italics().weak(),
                                );
                                ui.end_row();
                            }
                        }
                    }
                });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("OK"))
                        .clicked()
                    {
                        let mut map = HashMap::new();
                        for f in fields.iter() {
                            if let Some((k, v)) = f.collect() {
                                map.insert(k, v);
                            }
                        }
                        let _ = resp_tx.send(PluginResponse::FormResult(Some(map)));
                        closed = true;
                    }
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("Cancel"))
                        .clicked()
                    {
                        let _ = resp_tx.send(PluginResponse::FormResult(None));
                        closed = true;
                    }
                });
            });
        });

    closed
}

fn show_prompt(
    ctx: &egui::Context,
    message: &str,
    input: &mut String,
    resp_tx: &mpsc::UnboundedSender<PluginResponse>,
) -> bool {
    let mut closed = false;

    egui::Window::new("Plugin Input")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_size([350.0, 120.0])
        .show(ctx, |ui| {
            ui.label(message);
            ui.add_space(4.0);
            let resp = ui.add(
                crate::ui::widgets::text_edit(input).desired_width(ui.available_width()),
            );
            let enter = resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_sized(BTN_MIN_SIZE, egui::Button::new("OK")).clicked()
                        || enter
                    {
                        let _ = resp_tx.send(PluginResponse::Output(input.clone()));
                        closed = true;
                    }
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("Cancel"))
                        .clicked()
                    {
                        let _ = resp_tx.send(PluginResponse::Ok);
                        closed = true;
                    }
                });
            });
        });

    closed
}

fn show_confirm(
    ctx: &egui::Context,
    message: &str,
    resp_tx: &mpsc::UnboundedSender<PluginResponse>,
) -> bool {
    let mut closed = false;

    egui::Window::new("Confirm")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_size([350.0, 100.0])
        .show(ctx, |ui| {
            ui.label(message);
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("Yes"))
                        .clicked()
                    {
                        let _ = resp_tx.send(PluginResponse::Bool(true));
                        closed = true;
                    }
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("No"))
                        .clicked()
                    {
                        let _ = resp_tx.send(PluginResponse::Bool(false));
                        closed = true;
                    }
                });
            });
        });

    closed
}

fn show_alert(
    ctx: &egui::Context,
    title: &str,
    message: &str,
    resp_tx: &mpsc::UnboundedSender<PluginResponse>,
    is_error: bool,
) -> bool {
    let mut closed = false;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_size([350.0, 100.0])
        .show(ctx, |ui| {
            if is_error {
                ui.label(
                    egui::RichText::new(message)
                        .color(egui::Color32::from_rgb(220, 60, 60)),
                );
            } else {
                ui.label(message);
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("OK"))
                        .clicked()
                    {
                        let _ = resp_tx.send(PluginResponse::Ok);
                        closed = true;
                    }
                });
            });
        });

    closed
}

fn show_text_viewer(
    ctx: &egui::Context,
    title: &str,
    text: &str,
    copied_at: &mut Option<Instant>,
    resp_tx: &mpsc::UnboundedSender<PluginResponse>,
) -> bool {
    let mut closed = false;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_size([500.0, 400.0])
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(350.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut text.to_string())
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .interactive(false),
                    );
                });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("Close"))
                        .clicked()
                    {
                        let _ = resp_tx.send(PluginResponse::Ok);
                        closed = true;
                    }

                    // Show "Copied!" for 1.5s after copy, then revert to "Copy"
                    let recently_copied = copied_at
                        .map(|t| t.elapsed().as_secs_f32() < 1.5)
                        .unwrap_or(false);

                    let copy_label = if recently_copied { "Copied!" } else { "Copy" };
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new(copy_label))
                        .clicked()
                    {
                        ctx.copy_text(text.to_string());
                        *copied_at = Some(Instant::now());
                    }

                    // Request repaint while showing "Copied!" so it reverts
                    if recently_copied {
                        ctx.request_repaint();
                    }
                });
            });
        });

    closed
}

fn show_table_viewer(
    ctx: &egui::Context,
    title: &str,
    columns: &[String],
    rows: &[Vec<String>],
    resp_tx: &mpsc::UnboundedSender<PluginResponse>,
) -> bool {
    let mut closed = false;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_size([550.0, 400.0])
        .show(ctx, |ui| {
            let num_cols = columns.len();
            if num_cols > 0 {
                let mut builder = egui_extras::TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .max_scroll_height(350.0);

                for _ in 0..num_cols {
                    builder = builder.column(
                        egui_extras::Column::remainder()
                            .at_least(60.0)
                            .resizable(true),
                    );
                }

                builder
                    .header(20.0, |mut header| {
                        for col in columns {
                            header.col(|ui| {
                                ui.strong(col);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(18.0, rows.len(), |mut row| {
                            let idx = row.index();
                            if let Some(row_data) = rows.get(idx) {
                                for col_idx in 0..num_cols {
                                    row.col(|ui| {
                                        let val =
                                            row_data.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                                        ui.label(val);
                                    });
                                }
                            }
                        });
                    });
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new("Close"))
                        .clicked()
                    {
                        let _ = resp_tx.send(PluginResponse::Ok);
                        closed = true;
                    }
                });
            });
        });

    closed
}
