//! Shared UI widget helpers for consistent styling.

/// Build a `WidgetText` for a menu shortcut like "⌘T" with the ⌘ glyph
/// scaled down so it visually matches the key character height.
pub fn cmd_shortcut(key: &str) -> egui::WidgetText {
    use egui::text::LayoutJob;
    let key_size = 12.0;
    let cmd_size = key_size * 0.62;
    let mut job = LayoutJob::default();
    job.append(
        "\u{2318}",
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(cmd_size),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.append(
        key,
        1.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(key_size),
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job.into()
}

/// Standard inner margin for text edit fields.
pub const TEXT_EDIT_MARGIN: egui::Margin = egui::Margin {
    left: 4,
    right: 4,
    top: 4,
    bottom: 4,
};

/// Return a consistently styled single-line text edit.
///
/// Callers chain `.desired_width()`, `.hint_text()`, etc. as needed.
pub fn text_edit(buf: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(buf)
        .margin(TEXT_EDIT_MARGIN)
        .font(egui::TextStyle::Body)
}

/// Height of a single-line text edit (body font line height + vertical margin).
pub fn text_edit_height(ui: &egui::Ui) -> f32 {
    let font_height = ui.text_style_height(&egui::TextStyle::Body);
    font_height + TEXT_EDIT_MARGIN.top as f32 + TEXT_EDIT_MARGIN.bottom as f32
}

/// Minimum size for dialog action buttons.
pub const BTN_MIN_SIZE: egui::Vec2 = egui::Vec2::new(95.0, 26.0);

const BTN_FONT_SIZE: f32 = 14.0;

/// A consistently styled dialog button (Save, Cancel, OK, etc.).
pub fn dialog_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(BTN_MIN_SIZE, egui::Button::new(egui::RichText::new(label).size(BTN_FONT_SIZE)))
}

/// A consistently styled dialog button that can be disabled.
pub fn dialog_button_enabled(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(BTN_FONT_SIZE)).min_size(BTN_MIN_SIZE),
    )
}
