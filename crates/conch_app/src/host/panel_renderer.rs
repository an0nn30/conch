//! Plugin panel renderer — converts Widget trees to egui UI.
//!
//! Takes a `Vec<Widget>` from `conch_plugin_sdk::widgets` and renders each
//! widget into an egui `Ui`. Interactive widgets (buttons, text inputs,
//! checkboxes, combo boxes) generate `WidgetEvent`s that are returned to the
//! caller for delivery back to the plugin.

use std::collections::HashMap;

use conch_plugin_sdk::widgets::{BadgeVariant, TextStyle, Widget, WidgetEvent};
use egui::RichText;

use crate::ui_theme::UiTheme;

/// Render a list of widgets into an egui Ui, collecting widget events.
pub fn render_widgets(
    ui: &mut egui::Ui,
    widgets: &[Widget],
    theme: &UiTheme,
    text_input_state: &mut HashMap<String, String>,
) -> Vec<WidgetEvent> {
    let mut events = Vec::new();
    for widget in widgets {
        render_widget(ui, widget, theme, text_input_state, &mut events);
    }
    events
}

/// Render a single widget, recursing into layout containers.
fn render_widget(
    ui: &mut egui::Ui,
    widget: &Widget,
    theme: &UiTheme,
    text_input_state: &mut HashMap<String, String>,
    events: &mut Vec<WidgetEvent>,
) {
    match widget {
        // -- Layout Containers ------------------------------------------------

        Widget::Horizontal {
            children, spacing, ..
        } => {
            ui.horizontal(|ui| {
                if let Some(sp) = spacing {
                    ui.spacing_mut().item_spacing.x = *sp;
                }
                for child in children {
                    render_widget(ui, child, theme, text_input_state, events);
                }
            });
        }

        Widget::Vertical {
            children, spacing, ..
        } => {
            ui.vertical(|ui| {
                if let Some(sp) = spacing {
                    ui.spacing_mut().item_spacing.y = *sp;
                }
                for child in children {
                    render_widget(ui, child, theme, text_input_state, events);
                }
            });
        }

        Widget::ScrollArea {
            children,
            max_height,
            ..
        } => {
            let mut scroll = egui::ScrollArea::vertical();
            if let Some(h) = max_height {
                scroll = scroll.max_height(*h);
            }
            scroll.show(ui, |ui| {
                for child in children {
                    render_widget(ui, child, theme, text_input_state, events);
                }
            });
        }

        // -- Data Display -----------------------------------------------------

        Widget::Heading { text } => {
            ui.label(
                RichText::new(text)
                    .size(theme.font_normal + 2.0)
                    .strong()
                    .color(theme.text),
            );
        }

        Widget::Label { text, style } => {
            let color = text_style_color(style.as_ref(), theme);
            ui.label(RichText::new(text).size(theme.font_normal).color(color));
        }

        Widget::Text { text } => {
            ui.label(
                RichText::new(text)
                    .monospace()
                    .size(theme.font_small)
                    .color(theme.text),
            );
        }

        Widget::ScrollText {
            text, max_height, ..
        } => {
            let max_h = max_height.unwrap_or(200.0);
            egui::ScrollArea::vertical()
                .max_height(max_h)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(text)
                            .monospace()
                            .size(theme.font_small)
                            .color(theme.text),
                    );
                });
        }

        Widget::KeyValue { key, value } => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(key)
                        .color(theme.text_secondary)
                        .size(theme.font_small),
                );
                ui.label(
                    RichText::new(value)
                        .color(theme.text)
                        .size(theme.font_small),
                );
            });
        }

        Widget::Separator => {
            ui.separator();
        }

        Widget::Spacer { size } => {
            ui.add_space(size.unwrap_or(8.0));
        }

        Widget::IconLabel { text, style, .. } => {
            // MVP: render as a plain label, ignoring the icon.
            let color = text_style_color(style.as_ref(), theme);
            ui.label(RichText::new(text).size(theme.font_normal).color(color));
        }

        Widget::Badge { text, variant } => {
            let color = match variant {
                BadgeVariant::Info => theme.accent,
                BadgeVariant::Success => theme.accent,
                BadgeVariant::Warn => theme.warn,
                BadgeVariant::Error => theme.error,
            };
            ui.label(
                RichText::new(text)
                    .size(theme.font_small)
                    .strong()
                    .color(color),
            );
        }

        Widget::Progress {
            fraction, label, ..
        } => {
            let mut bar = egui::ProgressBar::new(*fraction);
            if let Some(lbl) = label {
                bar = bar.text(lbl.as_str());
            }
            ui.add(bar);
        }

        Widget::Image { .. } => {
            ui.label(
                RichText::new("[Image]")
                    .size(theme.font_small)
                    .color(theme.text_muted),
            );
        }

        // -- Interactive Widgets ----------------------------------------------

        Widget::Button {
            id,
            label,
            enabled,
            ..
        } => {
            let is_enabled = enabled.unwrap_or(true);
            let button = egui::Button::new(
                RichText::new(label)
                    .size(theme.font_normal)
                    .color(if is_enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    }),
            );
            ui.add_enabled_ui(is_enabled, |ui| {
                if ui.add(button).clicked() {
                    events.push(WidgetEvent::ButtonClick { id: id.clone() });
                }
            });
        }

        Widget::TextInput {
            id,
            value,
            hint,
            submit_on_enter,
        } => {
            // Initialize local edit buffer from the plugin's canonical value
            // if we haven't seen this widget before.
            let buf = text_input_state
                .entry(id.clone())
                .or_insert_with(|| value.clone());

            let mut te = egui::TextEdit::singleline(buf).font(egui::TextStyle::Body);
            if let Some(h) = hint {
                te = te.hint_text(h);
            }

            let response = ui.add(te);

            // Detect value change.
            if response.changed() {
                events.push(WidgetEvent::TextInputChanged {
                    id: id.clone(),
                    value: buf.clone(),
                });
            }

            // Detect Enter key submission.
            if submit_on_enter.unwrap_or(false) && response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                let submitted = buf.clone();
                // Clear local state so it re-syncs from the plugin next frame.
                text_input_state.remove(id);
                events.push(WidgetEvent::TextInputSubmit {
                    id: id.clone(),
                    value: submitted,
                });
            }
        }

        Widget::TextEdit {
            id, value, hint, lines,
        } => {
            let buf = text_input_state
                .entry(id.clone())
                .or_insert_with(|| value.clone());

            let desired_rows = lines.unwrap_or(4) as usize;
            let mut te = egui::TextEdit::multiline(buf)
                .font(egui::TextStyle::Monospace)
                .desired_rows(desired_rows);
            if let Some(h) = hint {
                te = te.hint_text(h);
            }

            let response = ui.add(te);
            if response.changed() {
                events.push(WidgetEvent::TextEditChanged {
                    id: id.clone(),
                    value: buf.clone(),
                });
            }
        }

        Widget::Checkbox {
            id,
            label,
            checked,
        } => {
            let mut val = *checked;
            if ui.checkbox(&mut val, label).changed() {
                events.push(WidgetEvent::CheckboxChanged {
                    id: id.clone(),
                    checked: val,
                });
            }
        }

        Widget::ComboBox {
            id,
            selected,
            options,
        } => {
            let mut current = selected.clone();
            egui::ComboBox::from_id_salt(id)
                .selected_text(&current)
                .show_ui(ui, |ui| {
                    for opt in options {
                        ui.selectable_value(&mut current, opt.value.clone(), &opt.label);
                    }
                });
            if current != *selected {
                events.push(WidgetEvent::ComboBoxChanged {
                    id: id.clone(),
                    value: current,
                });
            }
        }

        // -- Complex Widgets (MVP placeholders) -------------------------------

        Widget::SplitPane { left, right, .. } => {
            // MVP: render children sequentially with a separator.
            render_widget(ui, left, theme, text_input_state, events);
            ui.separator();
            render_widget(ui, right, theme, text_input_state, events);
        }

        Widget::Tabs { tabs, active, .. } => {
            // MVP: render only the active tab's children.
            if let Some(pane) = tabs.get(*active) {
                ui.label(
                    RichText::new(&pane.label)
                        .size(theme.font_small)
                        .strong()
                        .color(theme.text_secondary),
                );
                for child in &pane.children {
                    render_widget(ui, child, theme, text_input_state, events);
                }
            }
        }

        Widget::Toolbar { items, .. } => {
            ui.horizontal(|ui| {
                for item in items {
                    match item {
                        conch_plugin_sdk::widgets::ToolbarItem::Button { id, label, .. } => {
                            let text = label.as_deref().unwrap_or(id);
                            if ui.button(text).clicked() {
                                events.push(WidgetEvent::ButtonClick { id: id.clone() });
                            }
                        }
                        conch_plugin_sdk::widgets::ToolbarItem::Separator => {
                            ui.separator();
                        }
                        conch_plugin_sdk::widgets::ToolbarItem::Spacer => {
                            ui.add_space(8.0);
                        }
                        conch_plugin_sdk::widgets::ToolbarItem::TextInput {
                            id, value, hint,
                        } => {
                            let buf = text_input_state
                                .entry(id.clone())
                                .or_insert_with(|| value.clone());
                            let mut te = egui::TextEdit::singleline(buf)
                                .font(egui::TextStyle::Body)
                                .desired_width(120.0);
                            if let Some(h) = hint {
                                te = te.hint_text(h);
                            }
                            let response = ui.add(te);
                            if response.changed() {
                                events.push(WidgetEvent::ToolbarInputChanged {
                                    id: id.clone(),
                                    value: buf.clone(),
                                });
                            }
                            if response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                events.push(WidgetEvent::ToolbarInputSubmit {
                                    id: id.clone(),
                                    value: buf.clone(),
                                });
                            }
                        }
                    }
                }
            });
        }

        Widget::Table { .. } => {
            ui.label(
                RichText::new("[Table]")
                    .size(theme.font_small)
                    .color(theme.text_muted),
            );
        }

        Widget::TreeView { .. } => {
            ui.label(
                RichText::new("[TreeView]")
                    .size(theme.font_small)
                    .color(theme.text_muted),
            );
        }

        Widget::PathBar { segments, .. } => {
            ui.horizontal(|ui| {
                for segment in segments {
                    ui.label(
                        RichText::new(segment)
                            .size(theme.font_small)
                            .color(theme.text_secondary),
                    );
                    ui.label(
                        RichText::new("/")
                            .size(theme.font_small)
                            .color(theme.text_muted),
                    );
                }
            });
        }

        Widget::DropZone { label, children, .. } => {
            ui.group(|ui| {
                ui.label(
                    RichText::new(label)
                        .size(theme.font_small)
                        .color(theme.text_muted),
                );
                for child in children {
                    render_widget(ui, child, theme, text_input_state, events);
                }
            });
        }

        Widget::ContextMenu { child, .. } => {
            // MVP: just render the child, ignore context menu.
            render_widget(ui, child, theme, text_input_state, events);
        }
    }
}

/// Map an optional `TextStyle` to a theme color.
fn text_style_color(style: Option<&TextStyle>, theme: &UiTheme) -> egui::Color32 {
    match style {
        None | Some(TextStyle::Normal) => theme.text,
        Some(TextStyle::Secondary) => theme.text_secondary,
        Some(TextStyle::Muted) => theme.text_muted,
        Some(TextStyle::Accent) => theme.accent,
        Some(TextStyle::Warn) => theme.warn,
        Some(TextStyle::Error) => theme.error,
    }
}
