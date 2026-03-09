//! Notification history dialog — shows all session notifications in a table.

use std::time::SystemTime;

use egui::RichText;

use crate::notifications::{HistoryEntry, level_colors};

pub struct NotificationHistoryState {
    pub selected: Option<usize>,
}

impl NotificationHistoryState {
    pub fn new() -> Self {
        Self { selected: None }
    }
}

pub enum NotificationHistoryAction {
    None,
    Close,
}

/// Show the notification history dialog. Returns an action each frame.
pub fn show_notification_history(
    ctx: &egui::Context,
    state: &mut NotificationHistoryState,
    history: &[HistoryEntry],
) -> NotificationHistoryAction {
    let mut action = NotificationHistoryAction::None;

    // Handle keyboard navigation.
    ctx.input(|i| {
        if i.key_pressed(egui::Key::Escape) {
            action = NotificationHistoryAction::Close;
        }
        if i.key_pressed(egui::Key::ArrowDown) {
            if history.is_empty() {
                state.selected = None;
            } else {
                state.selected = Some(match state.selected {
                    Some(idx) => (idx + 1).min(history.len() - 1),
                    None => 0,
                });
            }
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            if history.is_empty() {
                state.selected = None;
            } else {
                state.selected = Some(match state.selected {
                    Some(idx) => idx.saturating_sub(1),
                    None => 0,
                });
            }
        }
    });

    if matches!(action, NotificationHistoryAction::Close) {
        return action;
    }

    egui::Window::new("Notification History")
        .collapsible(false)
        .resizable(true)
        .default_size([660.0, 420.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            if history.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.weak("No notifications yet.");
                });
                return;
            }

            let avail = ui.available_size();
            let table_height = (avail.y - 40.0).max(80.0);
            let dark_mode = ui.visuals().dark_mode;

            egui::ScrollArea::vertical()
                .max_height(table_height)
                .show(ui, |ui| {
                    let col_widths = column_widths(avail.x);

                    egui::Grid::new("notification_history_table")
                        .num_columns(4)
                        .spacing([0.0, 0.0])
                        .min_col_width(0.0)
                        .striped(true)
                        .show(ui, |ui| {
                            // Header row.
                            for (header, w) in ["", "Source", "Content", "Time"]
                                .iter()
                                .zip(col_widths.iter())
                            {
                                ui.add_sized(
                                    [*w, 22.0],
                                    egui::Label::new(RichText::new(*header).strong()),
                                );
                            }
                            ui.end_row();

                            // Data rows — newest first.
                            for (i, entry) in history.iter().enumerate().rev() {
                                let selected = state.selected == Some(i);
                                let (accent, _bg) = level_colors(entry.level, dark_mode);

                                let mut clicked = false;

                                // Severity dot column.
                                let level_label = match entry.level {
                                    conch_plugin::NotificationLevel::Info => "●",
                                    conch_plugin::NotificationLevel::Success => "●",
                                    conch_plugin::NotificationLevel::Warning => "▲",
                                    conch_plugin::NotificationLevel::Error => "✕",
                                };
                                let resp = ui.add_sized(
                                    [col_widths[0], 22.0],
                                    egui::SelectableLabel::new(
                                        selected,
                                        RichText::new(level_label).color(accent).size(11.0),
                                    ),
                                );
                                clicked |= resp.clicked();

                                // Source column.
                                let resp = ui.add_sized(
                                    [col_widths[1], 22.0],
                                    egui::SelectableLabel::new(
                                        selected,
                                        RichText::new(&entry.source).size(11.0),
                                    ),
                                );
                                clicked |= resp.clicked();

                                // Content column — truncate long messages.
                                let body = if entry.body.len() > 80 {
                                    format!("{}…", &entry.body[..79])
                                } else {
                                    entry.body.clone()
                                };
                                let resp = ui.add_sized(
                                    [col_widths[2], 22.0],
                                    egui::SelectableLabel::new(
                                        selected,
                                        RichText::new(body).size(11.0),
                                    ),
                                );
                                clicked |= resp.clicked();

                                // Time column.
                                let time_str = format_time(entry.timestamp);
                                let resp = ui.add_sized(
                                    [col_widths[3], 22.0],
                                    egui::SelectableLabel::new(
                                        selected,
                                        RichText::new(time_str).size(11.0).weak(),
                                    ),
                                );
                                clicked |= resp.clicked();

                                if clicked {
                                    state.selected = Some(i);
                                }
                                ui.end_row();
                            }
                        });
                });

            // Bottom button row.
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::ui::widgets::dialog_button(ui, "Close").clicked() {
                        action = NotificationHistoryAction::Close;
                    }
                });
            });
        });

    action
}

/// Compute column widths for the 4-column table.
fn column_widths(total: f32) -> [f32; 4] {
    let severity_w = 30.0;
    let time_w = 70.0;
    let remaining = (total - severity_w - time_w - 20.0).max(200.0);
    let source_w = remaining * 0.30;
    let content_w = remaining * 0.70;
    [severity_w, source_w, content_w, time_w]
}

/// Format a SystemTime as "HH:MM:SS".
fn format_time(time: SystemTime) -> String {
    let elapsed = SystemTime::now()
        .duration_since(time)
        .unwrap_or_default();
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}
