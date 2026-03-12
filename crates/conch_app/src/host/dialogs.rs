//! Plugin dialog rendering — form, prompt, confirm, alert, error dialogs.
//!
//! Plugins call blocking dialog functions (e.g., `HostApi::show_form`) from
//! their threads. The host renders the dialog in the egui update loop and
//! sends the result back via a oneshot channel.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

// ---------------------------------------------------------------------------
// Dialog request types (sent from plugin thread → UI thread)
// ---------------------------------------------------------------------------

/// A dialog request sent from a plugin thread to the UI thread.
pub enum DialogRequest {
    Form {
        descriptor: FormDescriptor,
        reply: oneshot::Sender<Option<String>>,
    },
    Confirm {
        msg: String,
        reply: oneshot::Sender<bool>,
    },
    Prompt {
        msg: String,
        default_value: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Alert {
        title: String,
        msg: String,
        reply: oneshot::Sender<()>,
    },
    Error {
        title: String,
        msg: String,
        reply: oneshot::Sender<()>,
    },
}

// ---------------------------------------------------------------------------
// Form descriptor (parsed from plugin JSON)
// ---------------------------------------------------------------------------

/// A form dialog descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormDescriptor {
    pub title: String,
    pub fields: Vec<FormField>,
}

/// A single field in a form dialog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FormField {
    Text {
        #[serde(alias = "name")]
        id: String,
        label: String,
        #[serde(default)]
        value: String,
    },
    Number {
        #[serde(alias = "name")]
        id: String,
        label: String,
        #[serde(default)]
        value: f64,
    },
    Combo {
        #[serde(alias = "name")]
        id: String,
        label: String,
        options: Vec<String>,
        #[serde(default)]
        value: String,
    },
    Checkbox {
        #[serde(alias = "name")]
        id: String,
        label: String,
        #[serde(default)]
        value: bool,
    },
    Separator,
    Label {
        text: String,
    },
}

// ---------------------------------------------------------------------------
// Dialog state (owned by the UI thread)
// ---------------------------------------------------------------------------

/// Manages dialog rendering in the egui update loop.
pub struct DialogState {
    /// Incoming dialog requests from plugin threads.
    rx: mpsc::UnboundedReceiver<DialogRequest>,
    /// Currently active dialog.
    active: Option<ActiveDialog>,
}

/// The sender side — given to the HostApi implementation.
pub type DialogSender = mpsc::UnboundedSender<DialogRequest>;

/// Create a dialog channel pair.
pub fn dialog_channel() -> (DialogSender, DialogState) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        tx,
        DialogState {
            rx,
            active: None,
        },
    )
}

enum ActiveDialog {
    Form {
        descriptor: FormDescriptor,
        values: HashMap<String, FormValue>,
        reply: oneshot::Sender<Option<String>>,
    },
    Confirm {
        msg: String,
        reply: oneshot::Sender<bool>,
    },
    Prompt {
        msg: String,
        value: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Alert {
        title: String,
        msg: String,
        reply: oneshot::Sender<()>,
    },
    Error {
        title: String,
        msg: String,
        reply: oneshot::Sender<()>,
    },
}

#[derive(Debug, Clone)]
enum FormValue {
    Text(String),
    Number(f64),
    Bool(bool),
}

impl DialogState {
    /// Call this in the egui update loop. Returns true if a dialog is active.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        // Check for new requests.
        if self.active.is_none() {
            if let Ok(request) = self.rx.try_recv() {
                self.active = Some(activate_request(request));
            }
        }

        let Some(dialog) = &mut self.active else {
            return false;
        };

        let mut should_close = false;

        match dialog {
            ActiveDialog::Form {
                descriptor,
                values,
                ..
            } => {
                should_close = show_form_dialog(ctx, descriptor, values);
            }
            ActiveDialog::Confirm { msg, .. } => {
                should_close = show_confirm_dialog(ctx, msg);
            }
            ActiveDialog::Prompt { msg, value, .. } => {
                should_close = show_prompt_dialog(ctx, msg, value);
            }
            ActiveDialog::Alert { title, msg, .. } => {
                should_close = show_alert_dialog(ctx, title, msg, false);
            }
            ActiveDialog::Error { title, msg, .. } => {
                should_close = show_alert_dialog(ctx, title, msg, true);
            }
        }

        if should_close {
            // Send the result and close.
            if let Some(dialog) = self.active.take() {
                send_dialog_result(dialog);
            }
        }

        self.active.is_some()
    }

    /// Whether a dialog is currently displayed.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }
}

fn activate_request(request: DialogRequest) -> ActiveDialog {
    match request {
        DialogRequest::Form { descriptor, reply } => {
            let values = initial_form_values(&descriptor);
            ActiveDialog::Form {
                descriptor,
                values,
                reply,
            }
        }
        DialogRequest::Confirm { msg, reply } => ActiveDialog::Confirm { msg, reply },
        DialogRequest::Prompt {
            msg,
            default_value,
            reply,
        } => ActiveDialog::Prompt {
            msg,
            value: default_value,
            reply,
        },
        DialogRequest::Alert { title, msg, reply } => ActiveDialog::Alert { title, msg, reply },
        DialogRequest::Error { title, msg, reply } => ActiveDialog::Error { title, msg, reply },
    }
}

fn initial_form_values(descriptor: &FormDescriptor) -> HashMap<String, FormValue> {
    let mut values = HashMap::new();
    for field in &descriptor.fields {
        match field {
            FormField::Text { id, value, .. } => {
                values.insert(id.clone(), FormValue::Text(value.clone()));
            }
            FormField::Number { id, value, .. } => {
                values.insert(id.clone(), FormValue::Number(*value));
            }
            FormField::Combo { id, value, .. } => {
                values.insert(id.clone(), FormValue::Text(value.clone()));
            }
            FormField::Checkbox { id, value, .. } => {
                values.insert(id.clone(), FormValue::Bool(*value));
            }
            FormField::Separator | FormField::Label { .. } => {}
        }
    }
    values
}

fn send_dialog_result(dialog: ActiveDialog) {
    match dialog {
        ActiveDialog::Form {
            values, reply, ..
        } => {
            let result = form_values_to_json(&values);
            let _ = reply.send(Some(result));
        }
        ActiveDialog::Confirm { reply, .. } => {
            // Default to false for close/cancel.
            let _ = reply.send(false);
        }
        ActiveDialog::Prompt { value, reply, .. } => {
            let _ = reply.send(Some(value));
        }
        ActiveDialog::Alert { reply, .. } | ActiveDialog::Error { reply, .. } => {
            let _ = reply.send(());
        }
    }
}

fn form_values_to_json(values: &HashMap<String, FormValue>) -> String {
    let mut map = serde_json::Map::new();
    for (k, v) in values {
        let json_val = match v {
            FormValue::Text(s) => serde_json::Value::String(s.clone()),
            FormValue::Number(n) => serde_json::json!(*n),
            FormValue::Bool(b) => serde_json::Value::Bool(*b),
        };
        map.insert(k.clone(), json_val);
    }
    serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

// ---------------------------------------------------------------------------
// Dialog rendering (egui)
// ---------------------------------------------------------------------------

/// Returns true if the dialog should close.
fn show_form_dialog(
    ctx: &egui::Context,
    descriptor: &FormDescriptor,
    values: &mut HashMap<String, FormValue>,
) -> bool {
    let mut close = false;
    let mut submitted = false;

    egui::Window::new(&descriptor.title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("form_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    for field in &descriptor.fields {
                        match field {
                            FormField::Text { id, label, .. } => {
                                ui.label(label);
                                if let Some(FormValue::Text(val)) = values.get_mut(id) {
                                    ui.text_edit_singleline(val);
                                }
                                ui.end_row();
                            }
                            FormField::Number { id, label, .. } => {
                                ui.label(label);
                                if let Some(FormValue::Number(val)) = values.get_mut(id) {
                                    ui.add(egui::DragValue::new(val));
                                }
                                ui.end_row();
                            }
                            FormField::Combo {
                                id,
                                label,
                                options,
                                ..
                            } => {
                                ui.label(label);
                                if let Some(FormValue::Text(selected)) = values.get_mut(id) {
                                    egui::ComboBox::from_id_salt(id)
                                        .selected_text(selected.as_str())
                                        .show_ui(ui, |ui| {
                                            for opt in options {
                                                ui.selectable_value(selected, opt.clone(), opt);
                                            }
                                        });
                                }
                                ui.end_row();
                            }
                            FormField::Checkbox { id, label, .. } => {
                                if let Some(FormValue::Bool(val)) = values.get_mut(id) {
                                    ui.label("");
                                    ui.checkbox(val, label);
                                }
                                ui.end_row();
                            }
                            FormField::Separator => {
                                ui.separator();
                                ui.separator();
                                ui.end_row();
                            }
                            FormField::Label { text } => {
                                ui.label("");
                                ui.label(text);
                                ui.end_row();
                            }
                        }
                    }
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button("OK").clicked() {
                    submitted = true;
                }
            });
        });

    if submitted {
        return true; // Close and send values.
    }
    if close {
        // Cancel — send null result.
        return true;
    }
    false
}

fn show_confirm_dialog(ctx: &egui::Context, msg: &str) -> bool {
    let mut result = false;

    egui::Window::new("Confirm")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(msg);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("No").clicked() {
                    result = true;
                }
                if ui.button("Yes").clicked() {
                    result = true;
                }
            });
        });

    result
}

fn show_prompt_dialog(ctx: &egui::Context, msg: &str, value: &mut String) -> bool {
    let mut close = false;

    egui::Window::new("Input")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(msg);
            let response = ui.text_edit_singleline(value);
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                close = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button("OK").clicked() {
                    close = true;
                }
            });
        });

    close
}

fn show_alert_dialog(ctx: &egui::Context, title: &str, msg: &str, is_error: bool) -> bool {
    let mut close = false;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            if is_error {
                ui.colored_label(egui::Color32::from_rgb(220, 60, 60), msg);
            } else {
                ui.label(msg);
            }
            ui.add_space(8.0);
            if ui.button("OK").clicked() {
                close = true;
            }
        });

    close
}

// ---------------------------------------------------------------------------
// Parsing helper
// ---------------------------------------------------------------------------

/// Parse a form descriptor from JSON (as sent by plugins).
pub fn parse_form_descriptor(json: &str) -> Option<FormDescriptor> {
    serde_json::from_str(json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_form_descriptor_basic() {
        let json = r#"{
            "title": "Add Server",
            "fields": [
                {"type": "text", "id": "host", "label": "Host", "value": ""},
                {"type": "number", "id": "port", "label": "Port", "value": 22},
                {"type": "combo", "id": "auth", "label": "Auth", "options": ["key", "password"], "value": "key"},
                {"type": "checkbox", "id": "save", "label": "Save password", "value": false},
                {"type": "separator"},
                {"type": "label", "text": "Optional settings below."}
            ]
        }"#;

        let desc = parse_form_descriptor(json).unwrap();
        assert_eq!(desc.title, "Add Server");
        assert_eq!(desc.fields.len(), 6);

        assert!(matches!(&desc.fields[0], FormField::Text { id, .. } if id == "host"));
        assert!(matches!(&desc.fields[1], FormField::Number { id, .. } if id == "port"));
        assert!(matches!(&desc.fields[2], FormField::Combo { id, options, .. } if id == "auth" && options.len() == 2));
        assert!(matches!(&desc.fields[3], FormField::Checkbox { id, .. } if id == "save"));
        assert!(matches!(&desc.fields[4], FormField::Separator));
        assert!(matches!(&desc.fields[5], FormField::Label { text } if text == "Optional settings below."));
    }

    #[test]
    fn initial_form_values_populated() {
        let desc = FormDescriptor {
            title: "Test".into(),
            fields: vec![
                FormField::Text {
                    id: "name".into(),
                    label: "Name".into(),
                    value: "default".into(),
                },
                FormField::Number {
                    id: "port".into(),
                    label: "Port".into(),
                    value: 22.0,
                },
                FormField::Checkbox {
                    id: "save".into(),
                    label: "Save".into(),
                    value: true,
                },
                FormField::Separator,
            ],
        };

        let values = initial_form_values(&desc);
        assert_eq!(values.len(), 3);
        assert!(matches!(values.get("name"), Some(FormValue::Text(s)) if s == "default"));
        assert!(matches!(values.get("port"), Some(FormValue::Number(n)) if (*n - 22.0).abs() < 0.01));
        assert!(matches!(values.get("save"), Some(FormValue::Bool(true))));
    }

    #[test]
    fn form_values_to_json_output() {
        let mut values = HashMap::new();
        values.insert("host".into(), FormValue::Text("example.com".into()));
        values.insert("port".into(), FormValue::Number(22.0));
        values.insert("save".into(), FormValue::Bool(true));

        let json = form_values_to_json(&values);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["host"], "example.com");
        assert_eq!(parsed["save"], true);
    }

    #[test]
    fn parse_form_with_name_alias() {
        let json = r#"{
            "title": "Test",
            "fields": [
                {"type": "text", "name": "host", "label": "Host", "value": "localhost"}
            ]
        }"#;
        let desc = parse_form_descriptor(json).unwrap();
        assert!(matches!(&desc.fields[0], FormField::Text { id, .. } if id == "host"));
    }

    #[test]
    fn dialog_channel_creates_pair() {
        let (tx, state) = dialog_channel();
        assert!(!state.is_active());
        // Sender should be usable.
        let (reply_tx, _reply_rx) = oneshot::channel();
        tx.send(DialogRequest::Alert {
            title: "Hi".into(),
            msg: "Test".into(),
            reply: reply_tx,
        })
        .unwrap();
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_form_descriptor("not json").is_none());
    }
}
