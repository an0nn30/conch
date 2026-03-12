//! SDK widget tree → egui rendering.
//!
//! Takes a `Vec<Widget>` from a plugin's render output and draws it using egui.
//! Collects user interactions as `WidgetEvent`s to send back to the plugin.

use std::collections::HashMap;

use conch_plugin_sdk::widgets::*;

/// Mutable state the renderer needs to track across frames for interactive
/// widgets (text inputs, checkboxes, etc.).
///
/// Each panel maintains its own `RendererState`.
#[derive(Debug, Default)]
pub struct RendererState {
    /// Text input / text edit values keyed by widget ID.
    pub text_values: HashMap<String, String>,
}

/// Render a widget tree into the given `egui::Ui`, collecting events.
///
/// Returns a list of `WidgetEvent`s triggered by user interaction this frame.
pub fn render_widgets(
    ui: &mut egui::Ui,
    widgets: &[Widget],
    state: &mut RendererState,
) -> Vec<WidgetEvent> {
    let mut events = Vec::new();
    for widget in widgets {
        render_one(ui, widget, state, &mut events);
    }
    events
}

fn render_one(
    ui: &mut egui::Ui,
    widget: &Widget,
    state: &mut RendererState,
    events: &mut Vec<WidgetEvent>,
) {
    match widget {
        // -- Layout ----------------------------------------------------------

        Widget::Horizontal {
            children, spacing, ..
        } => {
            ui.horizontal(|ui| {
                if let Some(sp) = spacing {
                    ui.spacing_mut().item_spacing.x = *sp;
                }
                for child in children {
                    render_one(ui, child, state, events);
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
                    render_one(ui, child, state, events);
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
                    render_one(ui, child, state, events);
                }
            });
        }

        Widget::Tabs { id, active, tabs } => {
            ui.horizontal(|ui| {
                for (i, tab) in tabs.iter().enumerate() {
                    let selected = i == *active;
                    if ui.selectable_label(selected, &tab.label).clicked() && !selected {
                        events.push(WidgetEvent::TabChanged {
                            id: id.clone(),
                            active: i,
                        });
                    }
                }
            });
            ui.separator();
            if let Some(tab) = tabs.get(*active) {
                for child in &tab.children {
                    render_one(ui, child, state, events);
                }
            }
        }

        Widget::SplitPane {
            direction,
            left,
            right,
            ..
        } => {
            // Simplified: render left then right in the appropriate direction.
            match direction {
                SplitDirection::Horizontal => {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| render_one(ui, left, state, events));
                        ui.separator();
                        ui.vertical(|ui| render_one(ui, right, state, events));
                    });
                }
                SplitDirection::Vertical => {
                    render_one(ui, left, state, events);
                    ui.separator();
                    render_one(ui, right, state, events);
                }
            }
        }

        // -- Data Display ----------------------------------------------------

        Widget::Heading { text } => {
            ui.heading(text);
        }

        Widget::Label { text, style } => {
            let rich = style_rich_text(egui::RichText::new(text), style.as_ref());
            ui.label(rich);
        }

        Widget::Text { text } => {
            ui.label(egui::RichText::new(text).monospace());
        }

        Widget::ScrollText {
            text, max_height, ..
        } => {
            let mut scroll = egui::ScrollArea::vertical().stick_to_bottom(true);
            if let Some(h) = max_height {
                scroll = scroll.max_height(*h);
            }
            scroll.show(ui, |ui| {
                ui.label(egui::RichText::new(text).monospace());
            });
        }

        Widget::KeyValue { key, value } => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(key).strong());
                ui.label(value);
            });
        }

        Widget::Separator => {
            ui.separator();
        }

        Widget::Spacer { size } => {
            if let Some(s) = size {
                ui.add_space(*s);
            } else {
                ui.add_space(ui.available_height().min(8.0));
            }
        }

        Widget::IconLabel { text, style, .. } => {
            let rich = style_rich_text(egui::RichText::new(text), style.as_ref());
            ui.label(rich);
        }

        Widget::Badge { text, variant } => {
            let color = badge_color(variant);
            let rich = egui::RichText::new(text).small().color(color);
            let frame = egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(6, 2))
                .corner_radius(4.0)
                .fill(color.linear_multiply(0.15));
            frame.show(ui, |ui| {
                ui.label(rich);
            });
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
            // Image rendering requires async loading — show placeholder.
            ui.label(egui::RichText::new("[image]").weak());
        }

        // -- Interactive Widgets ---------------------------------------------

        Widget::Button {
            id,
            label,
            enabled,
            ..
        } => {
            let btn = egui::Button::new(label);
            let enabled = enabled.unwrap_or(true);
            let response = ui.add_enabled(enabled, btn);
            if response.clicked() {
                events.push(WidgetEvent::ButtonClick { id: id.clone() });
            }
        }

        Widget::TextInput {
            id,
            value,
            hint,
            submit_on_enter,
        } => {
            let text = state
                .text_values
                .entry(id.clone())
                .or_insert_with(|| value.clone());

            let mut edit = egui::TextEdit::singleline(text).desired_width(f32::INFINITY);
            if let Some(h) = hint {
                edit = edit.hint_text(h);
            }
            let response = ui.add(edit);

            if response.changed() {
                events.push(WidgetEvent::TextInputChanged {
                    id: id.clone(),
                    value: text.clone(),
                });
            }
            if submit_on_enter.unwrap_or(true) && response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                events.push(WidgetEvent::TextInputSubmit {
                    id: id.clone(),
                    value: text.clone(),
                });
            }
        }

        Widget::TextEdit {
            id,
            value,
            hint,
            lines,
        } => {
            let text = state
                .text_values
                .entry(id.clone())
                .or_insert_with(|| value.clone());

            let rows = lines.unwrap_or(4) as usize;
            let mut edit = egui::TextEdit::multiline(text).desired_rows(rows);
            if let Some(h) = hint {
                edit = edit.hint_text(h);
            }
            let response = ui.add(edit);

            if response.changed() {
                events.push(WidgetEvent::TextEditChanged {
                    id: id.clone(),
                    value: text.clone(),
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
            egui::ComboBox::from_id_salt(id)
                .selected_text(
                    options
                        .iter()
                        .find(|o| o.value == *selected)
                        .map(|o| o.label.as_str())
                        .unwrap_or(selected.as_str()),
                )
                .show_ui(ui, |ui| {
                    for opt in options {
                        if ui
                            .selectable_label(opt.value == *selected, &opt.label)
                            .clicked()
                            && opt.value != *selected
                        {
                            events.push(WidgetEvent::ComboBoxChanged {
                                id: id.clone(),
                                value: opt.value.clone(),
                            });
                        }
                    }
                });
        }

        // -- Complex Widgets -------------------------------------------------

        Widget::Toolbar { items, .. } => {
            ui.horizontal(|ui| {
                for item in items {
                    match item {
                        ToolbarItem::Button {
                            id,
                            label,
                            tooltip,
                            enabled,
                            ..
                        } => {
                            let text = label.as_deref().unwrap_or("");
                            let btn = egui::Button::new(text);
                            let enabled = enabled.unwrap_or(true);
                            let response = ui.add_enabled(enabled, btn);
                            if let Some(tip) = tooltip {
                                response.clone().on_hover_text(tip);
                            }
                            if response.clicked() {
                                events.push(WidgetEvent::ButtonClick { id: id.clone() });
                            }
                        }
                        ToolbarItem::Separator => {
                            ui.separator();
                        }
                        ToolbarItem::Spacer => {
                            ui.add_space(ui.available_width().min(8.0));
                        }
                        ToolbarItem::TextInput { id, value, hint } => {
                            let text = state
                                .text_values
                                .entry(id.clone())
                                .or_insert_with(|| value.clone());

                            let mut edit = egui::TextEdit::singleline(text)
                                .desired_width(150.0);
                            if let Some(h) = hint {
                                edit = edit.hint_text(h);
                            }
                            let response = ui.add(edit);

                            if response.changed() {
                                events.push(WidgetEvent::ToolbarInputChanged {
                                    id: id.clone(),
                                    value: text.clone(),
                                });
                            }
                            if response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                events.push(WidgetEvent::ToolbarInputSubmit {
                                    id: id.clone(),
                                    value: text.clone(),
                                });
                            }
                        }
                    }
                }
            });
        }

        Widget::PathBar { id, segments } => {
            ui.horizontal(|ui| {
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0 {
                        ui.label("›");
                    }
                    if ui.link(seg).clicked() {
                        events.push(WidgetEvent::PathBarNavigate {
                            id: id.clone(),
                            segment_index: i,
                        });
                    }
                }
            });
        }

        Widget::TreeView {
            id,
            nodes,
            selected,
        } => {
            for node in nodes {
                render_tree_node(ui, id, node, selected.as_deref(), events);
            }
        }

        Widget::Table {
            id,
            columns,
            rows,
            sort_column,
            sort_ascending,
            selected_row,
        } => {
            render_table(
                ui,
                id,
                columns,
                rows,
                sort_column.as_deref(),
                *sort_ascending,
                selected_row.as_deref(),
                events,
            );
        }

        Widget::DropZone {
            children, label, ..
        } => {
            egui::Frame::NONE
                .stroke(egui::Stroke::new(1.0, egui::Color32::GRAY))
                .corner_radius(4.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    if children.is_empty() {
                        ui.label(egui::RichText::new(label).weak());
                    } else {
                        for child in children {
                            render_one(ui, child, state, events);
                        }
                    }
                });
        }

        Widget::ContextMenu { child, items } => {
            render_one(ui, child, state, events);
            // Context menu on the last response.
            ui.interact(
                ui.min_rect(),
                ui.id().with("ctx"),
                egui::Sense::click(),
            )
            .context_menu(|ui| {
                for item in items {
                    let enabled = item.enabled.unwrap_or(true);
                    if ui
                        .add_enabled(enabled, egui::Button::new(&item.label))
                        .clicked()
                    {
                        events.push(WidgetEvent::ContextMenuAction {
                            action: item.id.clone(),
                        });
                        ui.close_menu();
                    }
                }
            });
        }
    }
}

// -- Tree node rendering ---------------------------------------------------

fn render_tree_node(
    ui: &mut egui::Ui,
    tree_id: &str,
    node: &TreeNode,
    selected: Option<&str>,
    events: &mut Vec<WidgetEvent>,
) {
    let has_children = !node.children.is_empty();
    let is_selected = selected == Some(node.id.as_str());

    if has_children {
        let default_open = node.expanded.unwrap_or(false);
        let header_id = ui.id().with(&node.id);
        let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            header_id,
            default_open,
        );

        let header_response = ui.horizontal(|ui| {
            header.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
            let response = ui.selectable_label(is_selected, &node.label);
            if let Some(badge) = &node.badge {
                ui.label(
                    egui::RichText::new(badge)
                        .small()
                        .color(egui::Color32::from_rgb(100, 200, 100)),
                );
            }
            response
        });

        let label_response = header_response.inner;

        if label_response.clicked() {
            events.push(WidgetEvent::TreeSelect {
                id: tree_id.to_string(),
                node_id: node.id.clone(),
            });
        }
        if label_response.double_clicked() {
            events.push(WidgetEvent::TreeActivate {
                id: tree_id.to_string(),
                node_id: node.id.clone(),
            });
        }

        // Context menu on tree node.
        if let Some(menu_items) = &node.context_menu {
            label_response.context_menu(|ui| {
                render_tree_context_menu(ui, tree_id, &node.id, menu_items, events);
            });
        }

        header.show_body_unindented(ui, |ui| {
            ui.indent(&node.id, |ui| {
                for child in &node.children {
                    render_tree_node(ui, tree_id, child, selected, events);
                }
            });
        });
    } else {
        // Leaf node.
        let response = ui.horizontal(|ui| {
            ui.add_space(18.0); // Indent to match collapsing header toggle.
            let r = ui.selectable_label(is_selected, &node.label);
            if let Some(badge) = &node.badge {
                ui.label(
                    egui::RichText::new(badge)
                        .small()
                        .color(egui::Color32::from_rgb(100, 200, 100)),
                );
            }
            r
        });

        let label_response = response.inner;

        if label_response.clicked() {
            events.push(WidgetEvent::TreeSelect {
                id: tree_id.to_string(),
                node_id: node.id.clone(),
            });
        }
        if label_response.double_clicked() {
            events.push(WidgetEvent::TreeActivate {
                id: tree_id.to_string(),
                node_id: node.id.clone(),
            });
        }

        if let Some(menu_items) = &node.context_menu {
            label_response.context_menu(|ui| {
                render_tree_context_menu(ui, tree_id, &node.id, menu_items, events);
            });
        }
    }
}

fn render_tree_context_menu(
    ui: &mut egui::Ui,
    tree_id: &str,
    node_id: &str,
    items: &[ContextMenuItem],
    events: &mut Vec<WidgetEvent>,
) {
    for item in items {
        let enabled = item.enabled.unwrap_or(true);
        let mut btn = egui::Button::new(&item.label);
        if let Some(shortcut) = &item.shortcut {
            btn = btn.shortcut_text(shortcut);
        }
        if ui.add_enabled(enabled, btn).clicked() {
            events.push(WidgetEvent::TreeContextMenu {
                id: tree_id.to_string(),
                node_id: node_id.to_string(),
                action: item.id.clone(),
            });
            ui.close_menu();
        }
    }
}

// -- Table rendering -------------------------------------------------------

fn render_table(
    ui: &mut egui::Ui,
    table_id: &str,
    columns: &[TableColumn],
    rows: &[TableRow],
    sort_column: Option<&str>,
    sort_ascending: Option<bool>,
    selected_row: Option<&str>,
    events: &mut Vec<WidgetEvent>,
) {
    // Header row.
    ui.horizontal(|ui| {
        for col in columns {
            let is_sorted = sort_column == Some(col.id.as_str());
            let asc = sort_ascending.unwrap_or(true);
            let mut text = egui::RichText::new(&col.label).strong();
            if is_sorted {
                let arrow = if asc { " ▲" } else { " ▼" };
                text = egui::RichText::new(format!("{}{arrow}", col.label)).strong();
            }

            let sortable = col.sortable.unwrap_or(false);
            let width = col.width.unwrap_or(100.0);
            let response = ui.add_sized([width, ui.spacing().interact_size.y], egui::Label::new(text).sense(egui::Sense::click()));

            if sortable && response.clicked() {
                let new_asc = if is_sorted { !asc } else { true };
                events.push(WidgetEvent::TableSort {
                    id: table_id.to_string(),
                    column: col.id.clone(),
                    ascending: new_asc,
                });
            }
        }
    });

    ui.separator();

    // Data rows.
    for row in rows {
        let is_selected = selected_row == Some(row.id.as_str());

        let response = ui.horizontal(|ui| {
            for (i, cell) in row.cells.iter().enumerate() {
                let width = columns.get(i).and_then(|c| c.width).unwrap_or(100.0);
                let text = match cell {
                    TableCell::Text(t) => t.as_str(),
                    TableCell::Rich { text, .. } => text.as_str(),
                };
                ui.add_sized(
                    [width, ui.spacing().interact_size.y],
                    egui::SelectableLabel::new(is_selected, text),
                );
            }
        });

        if response.response.clicked() {
            events.push(WidgetEvent::TableSelect {
                id: table_id.to_string(),
                row_id: row.id.clone(),
            });
        }
        if response.response.double_clicked() {
            events.push(WidgetEvent::TableActivate {
                id: table_id.to_string(),
                row_id: row.id.clone(),
            });
        }

        if let Some(menu_items) = &row.context_menu {
            response.response.context_menu(|ui| {
                for item in menu_items {
                    let enabled = item.enabled.unwrap_or(true);
                    if ui
                        .add_enabled(enabled, egui::Button::new(&item.label))
                        .clicked()
                    {
                        events.push(WidgetEvent::TableContextMenu {
                            id: table_id.to_string(),
                            row_id: row.id.clone(),
                            action: item.id.clone(),
                        });
                        ui.close_menu();
                    }
                }
            });
        }
    }
}

// -- Helpers ---------------------------------------------------------------

fn style_rich_text(text: egui::RichText, style: Option<&TextStyle>) -> egui::RichText {
    match style {
        Some(TextStyle::Secondary) => text.weak(),
        Some(TextStyle::Muted) => text.weak().small(),
        Some(TextStyle::Accent) => text.color(egui::Color32::from_rgb(100, 150, 255)),
        Some(TextStyle::Warn) => text.color(egui::Color32::from_rgb(255, 180, 50)),
        Some(TextStyle::Error) => text.color(egui::Color32::from_rgb(255, 80, 80)),
        Some(TextStyle::Normal) | None => text,
    }
}

fn badge_color(variant: &BadgeVariant) -> egui::Color32 {
    match variant {
        BadgeVariant::Info => egui::Color32::from_rgb(100, 150, 255),
        BadgeVariant::Success => egui::Color32::from_rgb(80, 200, 100),
        BadgeVariant::Warn => egui::Color32::from_rgb(255, 180, 50),
        BadgeVariant::Error => egui::Color32::from_rgb(255, 80, 80),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_state_default_is_empty() {
        let state = RendererState::default();
        assert!(state.text_values.is_empty());
    }

    #[test]
    fn style_rich_text_normal() {
        let rt = style_rich_text(egui::RichText::new("test"), None);
        assert_eq!(rt.text(), "test");
    }

    #[test]
    fn style_rich_text_with_style() {
        let rt = style_rich_text(egui::RichText::new("warn"), Some(&TextStyle::Warn));
        assert_eq!(rt.text(), "warn");
    }

    #[test]
    fn badge_colors_distinct() {
        let info = badge_color(&BadgeVariant::Info);
        let success = badge_color(&BadgeVariant::Success);
        let warn = badge_color(&BadgeVariant::Warn);
        let error = badge_color(&BadgeVariant::Error);
        assert_ne!(info, success);
        assert_ne!(warn, error);
        assert_ne!(info, error);
    }
}
