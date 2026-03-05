//! Shared UI widget helpers for consistent styling.

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
