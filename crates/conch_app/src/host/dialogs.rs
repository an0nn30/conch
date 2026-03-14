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
    ContextMenu {
        items_json: String,
        reply: oneshot::Sender<Option<String>>,
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
    /// Custom buttons (default: Cancel + OK). Result JSON includes `"_action"`.
    #[serde(default)]
    pub buttons: Vec<FormButton>,
    /// Minimum dialog width in logical pixels.
    #[serde(default)]
    pub min_width: f32,
    /// Label column width in logical pixels (default 120).
    #[serde(default)]
    pub label_width: f32,
    /// If set, the dialog auto-closes with `_action: "_refresh"` after this
    /// many milliseconds, allowing the plugin to rebuild the form with fresh
    /// data and re-show it.
    #[serde(default)]
    pub auto_refresh_ms: u64,
}

/// A button in the form's action row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormButton {
    pub id: String,
    pub label: String,
    /// If set, button is only enabled when the named field is non-empty.
    #[serde(default)]
    pub enabled_when: Option<String>,
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
        #[serde(default)]
        hint: Option<String>,
    },
    Password {
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
    /// Host/IP + Port on the same row.
    HostPort {
        host_id: String,
        port_id: String,
        label: String,
        #[serde(default)]
        host_value: String,
        #[serde(default = "default_port")]
        port_value: String,
    },
    /// Text input with a "Browse..." file picker button.
    FilePicker {
        #[serde(alias = "name")]
        id: String,
        label: String,
        #[serde(default)]
        value: String,
        /// Starting directory for the file picker.
        #[serde(default)]
        start_dir: Option<String>,
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
    /// Collapsible section with nested fields.
    Collapsible {
        label: String,
        #[serde(default)]
        expanded: bool,
        fields: Vec<FormField>,
    },
    Separator,
    Label {
        text: String,
    },
    /// A read-only table with selectable rows. The selected row ID is stored
    /// in the form values under `id`. Each row has an `id` field that is
    /// returned when selected.
    SelectableTable {
        id: String,
        columns: Vec<String>,
        rows: Vec<SelectableTableRow>,
        #[serde(default)]
        value: String,
    },
}

/// A row in a `SelectableTable` form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectableTableRow {
    pub id: String,
    pub cells: Vec<String>,
    /// Optional color for the first cell (e.g. "green" for active status).
    #[serde(default)]
    pub color: Option<String>,
}

fn default_port() -> String {
    "22".to_string()
}

// ---------------------------------------------------------------------------
// Dialog state (owned by the UI thread)
// ---------------------------------------------------------------------------

/// A dialog request wrapped with the viewport it should appear in.
pub struct DialogMessage {
    pub request: DialogRequest,
    pub target_viewport: Option<egui::ViewportId>,
}

/// Manages dialog rendering in the egui update loop.
///
/// Supports multiple viewports — each viewport can have its own active dialog.
/// Incoming requests are routed to the correct viewport's queue based on
/// `DialogMessage::target_viewport`.
pub struct DialogState {
    /// Incoming dialog requests from plugin threads.
    rx: mpsc::UnboundedReceiver<DialogMessage>,
    /// Currently active dialog per viewport.
    active: HashMap<egui::ViewportId, ActiveDialog>,
    /// Pending requests queued per viewport (not yet activated).
    pending: HashMap<egui::ViewportId, Vec<DialogRequest>>,
}

/// The sender side — given to the HostApi implementation.
pub type DialogSender = mpsc::UnboundedSender<DialogMessage>;

/// Create a dialog channel pair.
pub fn dialog_channel() -> (DialogSender, DialogState) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        tx,
        DialogState {
            rx,
            active: HashMap::new(),
            pending: HashMap::new(),
        },
    )
}

enum ActiveDialog {
    Form {
        descriptor: FormDescriptor,
        values: HashMap<String, FormValue>,
        /// The button ID that was clicked (None = cancelled).
        action: Option<String>,
        reply: oneshot::Sender<Option<String>>,
        /// When the dialog was activated (for auto_refresh_ms).
        created_at: std::time::Instant,
    },
    Confirm {
        msg: String,
        confirmed: bool,
        reply: oneshot::Sender<bool>,
    },
    Prompt {
        msg: String,
        value: String,
        submitted: bool,
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
    ContextMenu {
        items: Vec<ContextMenuEntry>,
        selected: Option<String>,
        reply: oneshot::Sender<Option<String>>,
    },
}

/// A single entry in a plugin-requested context menu.
#[derive(Debug, Clone, serde::Deserialize)]
struct ContextMenuEntry {
    id: String,
    label: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
enum FormValue {
    Text(String),
    Number(f64),
    Bool(bool),
}

impl DialogState {
    /// Call this in the egui update loop for a specific viewport.
    /// Returns true if a dialog is active for this viewport.
    pub fn show(&mut self, ctx: &egui::Context, viewport_id: egui::ViewportId) -> bool {
        // Drain all new messages and route to per-viewport pending queues.
        while let Ok(msg) = self.rx.try_recv() {
            let vp = msg.target_viewport.unwrap_or(egui::ViewportId::ROOT);
            log::debug!(
                "DialogState::show: drained message target_viewport={:?} (raw={:?}), routing to {:?}",
                vp, msg.target_viewport, vp
            );
            self.pending.entry(vp).or_default().push(msg.request);
        }

        // If no active dialog for this viewport, try to activate one.
        if !self.active.contains_key(&viewport_id) {
            if let Some(queue) = self.pending.get_mut(&viewport_id) {
                if !queue.is_empty() {
                    let request = queue.remove(0);
                    log::info!(
                        "DialogState::show: activating dialog for viewport {:?}",
                        viewport_id
                    );
                    self.active.insert(viewport_id, activate_request(request));
                }
            }
        }

        let Some(dialog) = self.active.get_mut(&viewport_id) else {
            return false;
        };

        let mut should_close = false;

        match dialog {
            ActiveDialog::Form {
                descriptor,
                values,
                action,
                created_at,
                ..
            } => {
                should_close = show_form_dialog(ctx, descriptor, values, action, *created_at);
            }
            ActiveDialog::Confirm { msg, confirmed, .. } => {
                should_close = show_confirm_dialog(ctx, msg, confirmed);
            }
            ActiveDialog::Prompt { msg, value, submitted, .. } => {
                should_close = show_prompt_dialog(ctx, msg, value, submitted);
            }
            ActiveDialog::Alert { title, msg, .. } => {
                should_close = show_alert_dialog(ctx, title, msg, false);
            }
            ActiveDialog::Error { title, msg, .. } => {
                should_close = show_alert_dialog(ctx, title, msg, true);
            }
            ActiveDialog::ContextMenu { items, selected, .. } => {
                should_close = show_context_menu_dialog(ctx, items, selected);
            }
        }

        // ESC dismisses any active dialog (cancel result).
        if !should_close && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            should_close = true;
        }

        if should_close {
            if let Some(dialog) = self.active.remove(&viewport_id) {
                send_dialog_result(dialog);
            }
        }

        self.active.contains_key(&viewport_id)
    }

    /// Whether a dialog is currently displayed for the given viewport.
    pub fn is_active_for(&self, viewport_id: egui::ViewportId) -> bool {
        self.active.contains_key(&viewport_id)
    }
}

fn activate_request(request: DialogRequest) -> ActiveDialog {
    match request {
        DialogRequest::Form { descriptor, reply } => {
            let values = initial_form_values(&descriptor);
            ActiveDialog::Form {
                descriptor,
                values,
                action: None,
                reply,
                created_at: std::time::Instant::now(),
            }
        }
        DialogRequest::Confirm { msg, reply } => ActiveDialog::Confirm { msg, confirmed: false, reply },
        DialogRequest::Prompt {
            msg,
            default_value,
            reply,
        } => ActiveDialog::Prompt {
            msg,
            value: default_value,
            submitted: false,
            reply,
        },
        DialogRequest::Alert { title, msg, reply } => ActiveDialog::Alert { title, msg, reply },
        DialogRequest::Error { title, msg, reply } => ActiveDialog::Error { title, msg, reply },
        DialogRequest::ContextMenu { items_json, reply } => {
            let items: Vec<ContextMenuEntry> =
                serde_json::from_str(&items_json).unwrap_or_default();
            ActiveDialog::ContextMenu {
                items,
                selected: None,
                reply,
            }
        }
    }
}

fn initial_form_values(descriptor: &FormDescriptor) -> HashMap<String, FormValue> {
    let mut values = HashMap::new();
    collect_field_values(&descriptor.fields, &mut values);
    values
}

fn collect_field_values(fields: &[FormField], values: &mut HashMap<String, FormValue>) {
    for field in fields {
        match field {
            FormField::Text { id, value, .. } | FormField::Password { id, value, .. } => {
                values.insert(id.clone(), FormValue::Text(value.clone()));
            }
            FormField::Number { id, value, .. } => {
                values.insert(id.clone(), FormValue::Number(*value));
            }
            FormField::HostPort {
                host_id,
                port_id,
                host_value,
                port_value,
                ..
            } => {
                values.insert(host_id.clone(), FormValue::Text(host_value.clone()));
                values.insert(port_id.clone(), FormValue::Text(port_value.clone()));
            }
            FormField::FilePicker { id, value, .. } => {
                values.insert(id.clone(), FormValue::Text(value.clone()));
            }
            FormField::Combo { id, value, .. } => {
                values.insert(id.clone(), FormValue::Text(value.clone()));
            }
            FormField::Checkbox { id, value, .. } => {
                values.insert(id.clone(), FormValue::Bool(*value));
            }
            FormField::Collapsible { fields, expanded, .. } => {
                values.insert(
                    "__collapsible_expanded".to_string(),
                    FormValue::Bool(*expanded),
                );
                collect_field_values(fields, values);
            }
            FormField::SelectableTable { id, value, .. } => {
                values.insert(id.clone(), FormValue::Text(value.clone()));
            }
            FormField::Separator | FormField::Label { .. } => {}
        }
    }
}

fn send_dialog_result(dialog: ActiveDialog) {
    match dialog {
        ActiveDialog::Form {
            values, action, reply, ..
        } => {
            if let Some(action_id) = action {
                let result = form_values_to_json_with_action(&values, &action_id);
                let _ = reply.send(Some(result));
            } else {
                let _ = reply.send(None);
            }
        }
        ActiveDialog::Confirm { confirmed, reply, .. } => {
            let _ = reply.send(confirmed);
        }
        ActiveDialog::Prompt { value, submitted, reply, .. } => {
            if submitted {
                let _ = reply.send(Some(value));
            } else {
                let _ = reply.send(None);
            }
        }
        ActiveDialog::Alert { reply, .. } | ActiveDialog::Error { reply, .. } => {
            let _ = reply.send(());
        }
        ActiveDialog::ContextMenu { selected, reply, .. } => {
            let _ = reply.send(selected);
        }
    }
}

fn form_values_to_json(values: &HashMap<String, FormValue>) -> String {
    let mut map = serde_json::Map::new();
    for (k, v) in values {
        if k.starts_with("__") {
            continue; // Skip internal state keys.
        }
        let json_val = match v {
            FormValue::Text(s) => serde_json::Value::String(s.clone()),
            FormValue::Number(n) => serde_json::json!(*n),
            FormValue::Bool(b) => serde_json::Value::Bool(*b),
        };
        map.insert(k.clone(), json_val);
    }
    serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

fn form_values_to_json_with_action(values: &HashMap<String, FormValue>, action: &str) -> String {
    let mut map = serde_json::Map::new();
    for (k, v) in values {
        if k.starts_with("__") {
            continue;
        }
        let json_val = match v {
            FormValue::Text(s) => serde_json::Value::String(s.clone()),
            FormValue::Number(n) => serde_json::json!(*n),
            FormValue::Bool(b) => serde_json::Value::Bool(*b),
        };
        map.insert(k.clone(), json_val);
    }
    map.insert("_action".to_string(), serde_json::Value::String(action.to_string()));
    serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

// ---------------------------------------------------------------------------
// Dialog rendering (egui)
// ---------------------------------------------------------------------------

/// Button sizing constants matching main branch `widgets.rs`.
const BTN_MIN_SIZE: egui::Vec2 = egui::Vec2::new(95.0, 26.0);
const BTN_FONT_SIZE: f32 = 14.0;
const TEXT_EDIT_MARGIN: egui::Margin = egui::Margin {
    left: 8,
    right: 8,
    top: 6,
    bottom: 6,
};

/// Consistently styled text edit matching the main branch.
fn styled_text_edit(buf: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(buf)
        .margin(TEXT_EDIT_MARGIN)
        .font(egui::TextStyle::Body)
}

/// Check if a field value is non-empty (for `enabled_when` on buttons).
fn field_is_nonempty(values: &HashMap<String, FormValue>, field_id: &str) -> bool {
    match values.get(field_id) {
        Some(FormValue::Text(s)) => !s.trim().is_empty(),
        Some(FormValue::Number(_)) => true,
        Some(FormValue::Bool(_)) => true,
        None => false,
    }
}

/// Returns true if the dialog should close.
fn show_form_dialog(
    ctx: &egui::Context,
    descriptor: &FormDescriptor,
    values: &mut HashMap<String, FormValue>,
    action: &mut Option<String>,
    created_at: std::time::Instant,
) -> bool {
    // Auto-refresh: if enough time has elapsed, close with "_refresh" action.
    if descriptor.auto_refresh_ms > 0 {
        let elapsed = created_at.elapsed().as_millis() as u64;
        if elapsed >= descriptor.auto_refresh_ms {
            *action = Some("_refresh".to_string());
            return true;
        }
        // Request a repaint so we check again soon.
        ctx.request_repaint_after(std::time::Duration::from_millis(
            descriptor.auto_refresh_ms.saturating_sub(elapsed).max(50),
        ));
    }
    let mut close = false;
    let label_width = if descriptor.label_width > 0.0 {
        descriptor.label_width
    } else {
        120.0
    };

    let mut window = egui::Window::new(&descriptor.title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);

    if descriptor.min_width > 0.0 {
        // Use default_width + max_width to prevent the feedback loop where
        // desired_width(available_width()) causes infinite horizontal growth.
        window = window
            .default_width(descriptor.min_width)
            .min_width(descriptor.min_width)
            .max_width(descriptor.min_width);
    }

    window.show(ctx, |ui| {
        render_form_fields(ui, &descriptor.fields, values, label_width);

        ui.add_space(8.0);

        // Buttons.
        if descriptor.buttons.is_empty() {
            // Default Cancel / OK.
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new(
                            egui::RichText::new("OK").size(BTN_FONT_SIZE),
                        ))
                        .clicked()
                    {
                        *action = Some("ok".to_string());
                    }
                    if ui
                        .add_sized(BTN_MIN_SIZE, egui::Button::new(
                            egui::RichText::new("Cancel").size(BTN_FONT_SIZE),
                        ))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        } else {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Render buttons in reverse order (rightmost first in RTL layout).
                    for btn_def in descriptor.buttons.iter().rev() {
                        if btn_def.id == "cancel" {
                            if ui
                                .add_sized(BTN_MIN_SIZE, egui::Button::new(
                                    egui::RichText::new(&btn_def.label).size(BTN_FONT_SIZE),
                                ))
                                .clicked()
                            {
                                close = true;
                            }
                        } else {
                            let enabled = btn_def
                                .enabled_when
                                .as_ref()
                                .map(|f| field_is_nonempty(values, f))
                                .unwrap_or(true);
                            if ui
                                .add_enabled(
                                    enabled,
                                    egui::Button::new(
                                        egui::RichText::new(&btn_def.label).size(BTN_FONT_SIZE),
                                    )
                                    .min_size(BTN_MIN_SIZE),
                                )
                                .clicked()
                            {
                                *action = Some(btn_def.id.clone());
                            }
                        }
                    }
                });
            });
        }
    });

    if action.is_some() {
        return true;
    }
    close
}

/// Render form fields into an egui Grid.
///
/// Full-width fields (SelectableTable, Label with label_width=0) break out
/// of the grid and render at the full dialog width.
fn render_form_fields(
    ui: &mut egui::Ui,
    fields: &[FormField],
    values: &mut HashMap<String, FormValue>,
    label_width: f32,
) {
    // Split fields into segments: grid fields vs full-width fields.
    let mut i = 0;
    let mut grid_idx = 0;
    while i < fields.len() {
        // Collect consecutive grid-compatible fields.
        let start = i;
        while i < fields.len() && !is_full_width_field(&fields[i]) {
            i += 1;
        }
        if start < i {
            egui::Grid::new(format!("form_grid_{grid_idx}"))
                .num_columns(2)
                .spacing([8.0, 6.0])
                .min_col_width(label_width)
                .show(ui, |ui| {
                    for field in &fields[start..i] {
                        render_form_field(ui, field, values);
                    }
                });
            grid_idx += 1;
        }
        // Render full-width field(s).
        while i < fields.len() && is_full_width_field(&fields[i]) {
            render_full_width_field(ui, &fields[i], values);
            i += 1;
        }
    }
}

/// Whether a field should render at full dialog width outside the label/value grid.
fn is_full_width_field(field: &FormField) -> bool {
    matches!(field, FormField::SelectableTable { .. } | FormField::Label { .. })
}

/// Render a full-width field outside the form grid.
fn render_full_width_field(
    ui: &mut egui::Ui,
    field: &FormField,
    values: &mut HashMap<String, FormValue>,
) {
    match field {
        FormField::Label { text } => {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(text).weak());
            ui.add_space(2.0);
        }
        FormField::SelectableTable { id, columns, rows, .. } => {
            let selected = if let Some(FormValue::Text(val)) = values.get(id) {
                val.clone()
            } else {
                String::new()
            };

            // Table frame with background.
            let available_width = ui.available_width();
            egui::Frame::NONE
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(4.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.set_min_width(available_width - 8.0);
                    ui.set_min_height(120.0);

                    egui::ScrollArea::vertical()
                        .max_height(250.0)
                        .show(ui, |ui| {
                            egui::Grid::new(format!("selectable_table_{id}"))
                                .num_columns(columns.len())
                                .spacing([16.0, 2.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    // Header row.
                                    for col_name in columns {
                                        ui.strong(col_name);
                                    }
                                    ui.end_row();

                                    // Soften the selection highlight so text stays
                                    // readable against a dark background.
                                    ui.style_mut().visuals.selection.bg_fill =
                                        ui.visuals().selection.bg_fill.linear_multiply(0.25);

                                    // Data rows.
                                    for row_data in rows {
                                        let is_selected = row_data.id == selected;
                                        for (ci, cell) in row_data.cells.iter().enumerate() {
                                            let text = if ci == 0 {
                                                if let Some(color) = &row_data.color {
                                                    match color.as_str() {
                                                        "green" => egui::RichText::new(cell)
                                                            .color(egui::Color32::from_rgb(100, 200, 100)),
                                                        "red" => egui::RichText::new(cell)
                                                            .color(egui::Color32::from_rgb(200, 100, 100)),
                                                        "yellow" => egui::RichText::new(cell)
                                                            .color(egui::Color32::from_rgb(220, 200, 80)),
                                                        _ => egui::RichText::new(cell),
                                                    }
                                                } else {
                                                    egui::RichText::new(cell)
                                                }
                                            } else {
                                                egui::RichText::new(cell)
                                            };
                                            let resp = ui.selectable_label(is_selected, text);
                                            if resp.clicked() {
                                                if let Some(FormValue::Text(val)) =
                                                    values.get_mut(id)
                                                {
                                                    *val = row_data.id.clone();
                                                }
                                            }
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                });
            ui.add_space(4.0);
        }
        _ => {}
    }
}

/// Render a single form field row.
fn render_form_field(
    ui: &mut egui::Ui,
    field: &FormField,
    values: &mut HashMap<String, FormValue>,
) {
    match field {
        FormField::Text { id, label, hint, .. } => {
            ui.label(label);
            if let Some(FormValue::Text(val)) = values.get_mut(id) {
                let mut edit = styled_text_edit(val)
                    .desired_width(ui.available_width());
                if let Some(h) = hint {
                    edit = edit.hint_text(h);
                }
                ui.add(edit);
            }
            ui.end_row();
        }
        FormField::Password { id, label, .. } => {
            ui.label(label);
            if let Some(FormValue::Text(val)) = values.get_mut(id) {
                ui.add(
                    egui::TextEdit::singleline(val)
                        .password(true)
                        .margin(TEXT_EDIT_MARGIN)
                        .font(egui::TextStyle::Body)
                        .desired_width(ui.available_width()),
                );
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
        FormField::HostPort {
            host_id,
            port_id,
            label,
            ..
        } => {
            ui.label(label);
            ui.horizontal(|ui| {
                if let Some(FormValue::Text(host_val)) = values.get_mut(host_id) {
                    ui.add(
                        styled_text_edit(host_val)
                            .desired_width(ui.available_width() - 80.0),
                    );
                }
                ui.label(":");
                if let Some(FormValue::Text(port_val)) = values.get_mut(port_id) {
                    ui.add(styled_text_edit(port_val).desired_width(50.0));
                }
            });
            ui.end_row();
        }
        FormField::FilePicker {
            id,
            label,
            start_dir,
            ..
        } => {
            ui.label(label);
            ui.horizontal(|ui| {
                if let Some(FormValue::Text(val)) = values.get_mut(id) {
                    ui.add(
                        styled_text_edit(val)
                            .desired_width(ui.available_width() - 80.0),
                    );
                    if ui
                        .add_sized(
                            [70.0, BTN_MIN_SIZE.y],
                            egui::Button::new(
                                egui::RichText::new("Browse\u{2026}").size(BTN_FONT_SIZE),
                            ),
                        )
                        .clicked()
                    {
                        let mut dialog = rfd::FileDialog::new();
                        let dir = start_dir.as_deref().unwrap_or("~/.ssh");
                        let expanded = if dir.starts_with("~/") {
                            dirs::home_dir()
                                .unwrap_or_default()
                                .join(&dir[2..])
                        } else {
                            std::path::PathBuf::from(dir)
                        };
                        if expanded.is_dir() {
                            dialog = dialog.set_directory(&expanded);
                        }
                        if let Some(path) = dialog.pick_file() {
                            *val = path.display().to_string();
                        }
                    }
                }
            });
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
                    .width(ui.available_width())
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
        FormField::Collapsible {
            label,
            fields,
            ..
        } => {
            // End the current grid, render collapsible outside it, then continue.
            // Since we're inside a grid, we span across both columns.
            let is_expanded = match values.get("__collapsible_expanded") {
                Some(FormValue::Bool(b)) => *b,
                _ => false,
            };
            let arrow = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
            let header = format!("{arrow} {label}");
            if ui
                .add(egui::Label::new(&header).sense(egui::Sense::click()))
                .clicked()
            {
                values.insert(
                    "__collapsible_expanded".to_string(),
                    FormValue::Bool(!is_expanded),
                );
            }
            // Empty second column.
            ui.label("");
            ui.end_row();

            if is_expanded {
                // Render nested fields in the same grid — they'll get the
                // same label/value column layout as the outer fields.
                for child in fields {
                    // Indent the label by prefixing with spaces.
                    render_form_field(ui, child, values);
                }
            }
        }
        FormField::Separator => {
            ui.separator();
            ui.separator();
            ui.end_row();
        }
        // Label and SelectableTable are full-width fields rendered outside the grid.
        FormField::Label { .. } | FormField::SelectableTable { .. } => {
            unreachable!("full-width fields are handled by render_full_width_field");
        }
    }
}

fn show_confirm_dialog(ctx: &egui::Context, msg: &str, confirmed: &mut bool) -> bool {
    let mut close = false;

    egui::Window::new("Confirm")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(msg);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("No").clicked() {
                    *confirmed = false;
                    close = true;
                }
                if ui.button("Yes").clicked() {
                    *confirmed = true;
                    close = true;
                }
            });
        });

    close
}

fn show_prompt_dialog(ctx: &egui::Context, msg: &str, value: &mut String, submitted: &mut bool) -> bool {
    let mut close = false;

    egui::Window::new("Input")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(msg);
            let is_password = msg.to_lowercase().contains("password");
            let mut edit = egui::TextEdit::singleline(value)
                .margin(TEXT_EDIT_MARGIN)
                .font(egui::TextStyle::Body);
            if is_password {
                edit = edit.password(true);
            }
            let response = ui.add(edit);
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                *submitted = true;
                close = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button("OK").clicked() {
                    *submitted = true;
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

fn show_context_menu_dialog(
    ctx: &egui::Context,
    items: &[ContextMenuEntry],
    selected: &mut Option<String>,
) -> bool {
    let mut close = false;

    // Show as a small floating window at the cursor position.
    let pos = ctx.input(|i| i.pointer.hover_pos().unwrap_or_default());
    egui::Area::new(egui::Id::new("plugin_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for item in items {
                    let btn = egui::Button::new(&item.label).frame(false);
                    let response = ui.add_enabled(item.enabled, btn);
                    if response.clicked() {
                        *selected = Some(item.id.clone());
                        close = true;
                    }
                }
            });
        });

    // Click outside → dismiss.
    if !close && ctx.input(|i| i.pointer.any_click()) {
        let area_rect = ctx
            .memory(|m| m.area_rect(egui::Id::new("plugin_context_menu")));
        if let Some(rect) = area_rect {
            if !rect.contains(ctx.input(|i| i.pointer.hover_pos().unwrap_or_default())) {
                close = true;
            }
        }
    }

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
                    hint: None,
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
            buttons: Vec::new(),
            min_width: 0.0,
            label_width: 0.0,
            auto_refresh_ms: 0,
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
        assert!(!state.is_active_for(egui::ViewportId::ROOT));
        // Sender should be usable.
        let (reply_tx, _reply_rx) = oneshot::channel();
        tx.send(DialogMessage {
            request: DialogRequest::Alert {
                title: "Hi".into(),
                msg: "Test".into(),
                reply: reply_tx,
            },
            target_viewport: None,
        })
        .unwrap();
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_form_descriptor("not json").is_none());
    }
}
