//! Centralized UI theme engine.
//!
//! Generates a consistent visual style across the entire application from
//! the terminal color scheme. Every widget — built-in egui or custom —
//! pulls its colors and metrics from here.
//!
//! Designed for plugin SDK access: plugins will be able to query the
//! theme to render UI that matches the host application.

use conch_core::config::{AppearanceMode, UiFontConfig};
use egui::{
    Color32, CornerRadius, Shadow, Stroke, Visuals,
    style::{WidgetVisuals, Widgets},
};

use crate::terminal::color::ResolvedColors;

/// Pre-computed UI theme derived from the terminal color scheme.
///
/// Created once (and recreated on theme change), then shared by reference
/// across all rendering code.
#[derive(Debug, Clone)]
pub struct UiTheme {
    // ── Core palette ──
    /// Terminal background.
    pub bg: Color32,
    /// Slightly elevated surface (panels, tab bar, menu bar).
    pub surface: Color32,
    /// More elevated surface (active tab, hovered items).
    pub surface_raised: Color32,
    /// Highest elevation surface (pressed/active elements).
    pub surface_top: Color32,
    /// Primary text color (terminal foreground).
    pub text: Color32,
    /// Secondary/dimmed text.
    pub text_secondary: Color32,
    /// Very dim text or subtle dividers.
    pub text_muted: Color32,
    /// Accent color for highlights, active indicators, links.
    pub accent: Color32,
    /// Soft focus glow color (sky blue).
    pub focus_glow: Color32,
    /// Divider/border color.
    pub border: Color32,
    /// Warning color.
    pub warn: Color32,
    /// Error color.
    pub error: Color32,

    // ── Metrics ──
    /// Standard corner rounding for widgets.
    pub rounding: u8,
    /// Small font size (labels, tab titles).
    pub font_small: f32,
    /// List item font size (tree nodes, table rows).
    pub font_list: f32,
    /// Normal font size (body text, buttons).
    pub font_normal: f32,
    /// Minimum width for menu popups (menu bar and context menus).
    pub menu_width: f32,

    // ── Mode ──
    /// Whether we are in dark mode.
    pub dark_mode: bool,
}

impl UiTheme {
    /// Build a theme from the terminal color scheme, appearance mode, and UI font config.
    pub fn from_colors(colors: &ResolvedColors, appearance: AppearanceMode, ui_font: &UiFontConfig) -> Self {
        let bg = to_color32(colors.background);
        let fg = to_color32(colors.foreground);

        let bg_r = bg.r();
        let bg_g = bg.g();
        let bg_b = bg.b();

        // For System mode, infer dark/light from background luminance.
        let dark_mode = match appearance {
            AppearanceMode::Dark => true,
            AppearanceMode::Light => false,
            AppearanceMode::System => {
                let luminance = 0.299 * bg_r as f32 + 0.587 * bg_g as f32 + 0.114 * bg_b as f32;
                luminance < 128.0
            }
        };

        // Use explicit base colors for UI chrome.
        // The terminal area still uses the color scheme's background.
        let (ui_bg, surface, text_primary, text_secondary, text_muted, focus_glow, border) =
            if dark_mode {
                (
                    Color32::from_rgb(0x20, 0x1E, 0x1F), // #201E1F
                    Color32::from_rgb(0x29, 0x28, 0x29), // #292829
                    Color32::from_rgb(0xE8, 0xE6, 0xE3), // warm white text
                    Color32::from_rgb(0xA0, 0x9C, 0x9D), // warm gray
                    Color32::from_rgb(0x5A, 0x57, 0x58),  // warm muted
                    Color32::from_rgb(100, 160, 230),      // soft sky blue
                    Color32::from_rgb(0x48, 0x45, 0x46),  // dark border
                )
            } else {
                (
                    Color32::from_rgb(0xF0, 0xF0, 0xF0), // light gray bg
                    Color32::from_rgb(0xFA, 0xFA, 0xFA), // near-white surface
                    Color32::from_rgb(0x1A, 0x1A, 0x1A), // near-black text
                    Color32::from_rgb(0x60, 0x60, 0x60), // medium gray
                    Color32::from_rgb(0xA0, 0xA0, 0xA0), // light muted
                    Color32::from_rgb(80, 140, 210),       // blue focus
                    Color32::from_rgb(0xCC, 0xCC, 0xCC),  // light border
                )
            };

        let surface_raised = offset_color(surface, dark_mode, 12);
        let surface_top = offset_color(surface, dark_mode, 24);

        Self {
            bg: ui_bg,
            surface,
            surface_raised,
            surface_top,
            text: text_primary,
            text_secondary,
            text_muted,
            accent: to_color32(colors.normal[4]), // blue
            focus_glow,
            border,
            warn: to_color32(colors.normal[3]),  // yellow
            error: to_color32(colors.normal[1]), // red
            rounding: 0,
            font_small: ui_font.small,
            font_list: ui_font.list,
            font_normal: ui_font.normal,
            menu_width: 120.0,
            dark_mode,
        }
    }

    /// Generate egui `Visuals` from this theme so built-in widgets
    /// (buttons, menus, text inputs, etc.) automatically match.
    pub fn to_visuals(&self) -> Visuals {
        let rounding = CornerRadius::same(self.rounding);

        let noninteractive = WidgetVisuals {
            bg_fill: self.surface,
            weak_bg_fill: self.surface,
            bg_stroke: Stroke::new(1.0, self.border),
            corner_radius: rounding,
            fg_stroke: Stroke::new(1.0, self.text_secondary),
            expansion: 0.0,
        };

        let inactive = WidgetVisuals {
            bg_fill: if self.dark_mode { self.surface } else { Color32::WHITE },
            weak_bg_fill: self.surface,
            bg_stroke: if self.dark_mode {
                Stroke::new(1.0, self.border)
            } else {
                Stroke::new(1.0, Color32::from_rgb(0x30, 0x30, 0x30))
            },
            corner_radius: rounding,
            fg_stroke: Stroke::new(1.0, self.text),
            expansion: 0.0,
        };

        let hovered = WidgetVisuals {
            bg_fill: self.surface_raised,
            weak_bg_fill: self.surface_raised,
            bg_stroke: Stroke::new(1.0, self.focus_glow),
            corner_radius: rounding,
            fg_stroke: Stroke::new(1.0, self.text),
            expansion: 0.0,
        };

        let active = WidgetVisuals {
            bg_fill: self.surface_top,
            weak_bg_fill: self.surface_top,
            bg_stroke: Stroke::new(1.0, self.focus_glow),
            corner_radius: rounding,
            fg_stroke: Stroke::new(1.0, self.text),
            expansion: 0.0,
        };

        let open = WidgetVisuals {
            bg_fill: self.surface_raised,
            weak_bg_fill: self.surface_raised,
            bg_stroke: Stroke::new(1.0, self.border),
            corner_radius: rounding,
            fg_stroke: Stroke::new(1.0, self.text),
            expansion: 0.0,
        };

        Visuals {
            dark_mode: self.dark_mode,
            override_text_color: None,
            widgets: Widgets {
                noninteractive,
                inactive,
                hovered,
                active,
                open,
            },
            selection: egui::style::Selection {
                bg_fill: self.accent.linear_multiply(0.4),
                stroke: Stroke::new(1.0, self.accent),
            },
            hyperlink_color: self.accent,
            faint_bg_color: self.surface,
            extreme_bg_color: self.bg,
            code_bg_color: self.surface,
            warn_fg_color: self.warn,
            error_fg_color: self.error,
            window_corner_radius: CornerRadius::ZERO,
            window_shadow: Shadow {
                offset: [1, 1],
                blur: 0,
                spread: 0,
                color: Color32::from_black_alpha(80),
            },
            window_fill: self.surface,
            window_stroke: Stroke::new(1.0, self.border),
            window_highlight_topmost: true,
            menu_corner_radius: CornerRadius::ZERO,
            panel_fill: self.bg,
            popup_shadow: Shadow {
                offset: [1, 1],
                blur: 0,
                spread: 0,
                color: Color32::from_black_alpha(80),
            },
            resize_corner_size: 12.0,
            clip_rect_margin: 3.0,
            button_frame: true,
            collapsing_header_frame: false,
            indent_has_left_vline: true,
            striped: false,
            slider_trailing_fill: true,
            handle_shape: egui::style::HandleShape::Circle,
            interact_cursor: None,
            image_loading_spinners: true,
            numeric_color_space: egui::style::NumericColorSpace::GammaByte,
            text_cursor: Default::default(),
        }
    }

    /// Return the background color with a custom alpha (0–255).
    /// Useful for translucent overlays like the buttonless drag region.
    pub fn bg_with_alpha(&self, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(self.bg.r(), self.bg.g(), self.bg.b(), alpha)
    }

    /// Standard inner margin for text edit widgets.
    ///
    /// Use this with `TextEdit::singleline(buf).margin(theme.text_edit_margin())`
    /// so all text inputs share consistent padding.
    pub fn text_edit_margin(&self) -> egui::Margin {
        egui::Margin::symmetric(8, 8)
    }

    /// Apply this theme to an egui context.
    ///
    /// Sets both `Visuals` (colors, rounding, shadows) and `Spacing`
    /// (menu width). This covers all egui popups — menu bar dropdowns
    /// and right-click context menus use the same pipeline.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        style.visuals = self.to_visuals();
        style.spacing.menu_width = self.menu_width;

        // Compact padding for a flat, utilitarian feel.
        style.spacing.button_padding = egui::vec2(6.0, 4.0);
        style.spacing.interact_size.y = 26.0;

        // Override the default body font size.
        use egui::{FontId, TextStyle};
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(self.font_normal));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(self.font_normal));

        ctx.set_style(style);
    }
}

/// Offset a color by `amount` — lighten in dark mode, darken in light mode.
fn offset_color(base: Color32, dark_mode: bool, amount: u8) -> Color32 {
    if dark_mode {
        Color32::from_rgb(
            base.r().saturating_add(amount),
            base.g().saturating_add(amount),
            base.b().saturating_add(amount),
        )
    } else {
        Color32::from_rgb(
            base.r().saturating_sub(amount),
            base.g().saturating_sub(amount),
            base.b().saturating_sub(amount),
        )
    }
}

/// Convert `[f32; 4]` RGBA to `Color32`.
fn to_color32(c: [f32; 4]) -> Color32 {
    Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::color::ResolvedColors;

    fn dark_colors() -> ResolvedColors {
        // Dracula-like dark scheme.
        ResolvedColors {
            background: [0x28 as f32 / 255.0, 0x2a as f32 / 255.0, 0x36 as f32 / 255.0, 1.0],
            foreground: [0xf8 as f32 / 255.0, 0xf8 as f32 / 255.0, 0xf2 as f32 / 255.0, 1.0],
            normal: [
                [0.0, 0.0, 0.0, 1.0],
                [1.0, 0.33, 0.33, 1.0], // red
                [0.31, 0.98, 0.48, 1.0], // green
                [0.95, 0.98, 0.48, 1.0], // yellow
                [0.39, 0.58, 0.93, 1.0], // blue
                [0.74, 0.58, 0.98, 1.0], // magenta
                [0.54, 0.98, 0.98, 1.0], // cyan
                [1.0, 1.0, 1.0, 1.0],
            ],
            bright: [[0.5; 4]; 8],
            dim: [[0.2; 4]; 8],
            cursor_color: None,
            selection_text: None,
            selection_bg: None,
            bright_foreground: None,
            dim_foreground: None,
        }
    }

    fn light_colors() -> ResolvedColors {
        ResolvedColors {
            background: [0.95, 0.95, 0.95, 1.0], // Very light gray.
            foreground: [0.1, 0.1, 0.1, 1.0],
            normal: [
                [0.0, 0.0, 0.0, 1.0],
                [0.8, 0.0, 0.0, 1.0],
                [0.0, 0.6, 0.0, 1.0],
                [0.8, 0.6, 0.0, 1.0],
                [0.0, 0.0, 0.8, 1.0],
                [0.6, 0.0, 0.6, 1.0],
                [0.0, 0.6, 0.6, 1.0],
                [0.9, 0.9, 0.9, 1.0],
            ],
            bright: [[0.5; 4]; 8],
            dim: [[0.2; 4]; 8],
            cursor_color: None,
            selection_text: None,
            selection_bg: None,
            bright_foreground: None,
            dim_foreground: None,
        }
    }

    // -- to_color32 --

    #[test]
    fn to_color32_black() {
        assert_eq!(to_color32([0.0, 0.0, 0.0, 1.0]), Color32::from_rgb(0, 0, 0));
    }

    #[test]
    fn to_color32_white() {
        assert_eq!(to_color32([1.0, 1.0, 1.0, 1.0]), Color32::from_rgb(255, 255, 255));
    }

    #[test]
    fn to_color32_midpoint() {
        let c = to_color32([0.5, 0.5, 0.5, 1.0]);
        assert_eq!(c.r(), 127);
        assert_eq!(c.g(), 127);
        assert_eq!(c.b(), 127);
    }

    // -- offset_color --

    #[test]
    fn offset_color_dark_mode_lightens() {
        let base = Color32::from_rgb(100, 100, 100);
        let result = offset_color(base, true, 30);
        assert_eq!(result.r(), 130);
        assert_eq!(result.g(), 130);
        assert_eq!(result.b(), 130);
    }

    #[test]
    fn offset_color_light_mode_darkens() {
        let base = Color32::from_rgb(200, 200, 200);
        let result = offset_color(base, false, 30);
        assert_eq!(result.r(), 170);
        assert_eq!(result.g(), 170);
        assert_eq!(result.b(), 170);
    }

    #[test]
    fn offset_color_saturates_at_255() {
        let base = Color32::from_rgb(250, 250, 250);
        let result = offset_color(base, true, 30);
        assert_eq!(result.r(), 255);
    }

    #[test]
    fn offset_color_saturates_at_0() {
        let base = Color32::from_rgb(10, 10, 10);
        let result = offset_color(base, false, 30);
        assert_eq!(result.r(), 0);
    }

    // -- UiTheme::from_colors --

    #[test]
    fn from_colors_dark_mode_explicit() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        assert!(theme.dark_mode);
    }

    #[test]
    fn from_colors_light_mode_explicit() {
        let theme = UiTheme::from_colors(&light_colors(), AppearanceMode::Light, &UiFontConfig::default());
        assert!(!theme.dark_mode);
    }

    #[test]
    fn from_colors_system_infers_dark() {
        // Dracula background is dark → luminance < 128.
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::System, &UiFontConfig::default());
        assert!(theme.dark_mode);
    }

    #[test]
    fn from_colors_system_infers_light() {
        // Light background → luminance > 128.
        let theme = UiTheme::from_colors(&light_colors(), AppearanceMode::System, &UiFontConfig::default());
        assert!(!theme.dark_mode);
    }

    #[test]
    fn from_colors_surfaces_lighter_in_dark_mode() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        // Surface should be brighter than bg.
        assert!(theme.surface.r() > theme.bg.r());
        // surface_raised brighter than surface.
        assert!(theme.surface_raised.r() > theme.surface.r());
        // surface_top brighter than surface_raised.
        assert!(theme.surface_top.r() > theme.surface_raised.r());
    }

    #[test]
    fn from_colors_light_mode_has_light_surfaces() {
        let theme = UiTheme::from_colors(&light_colors(), AppearanceMode::Light, &UiFontConfig::default());
        // Light mode uses explicit light colors regardless of terminal scheme.
        assert!(theme.bg.r() > 200);
        assert!(theme.surface.r() > 200);
        assert!(!theme.dark_mode);
    }

    #[test]
    fn from_colors_accent_is_blue() {
        let colors = dark_colors();
        let theme = UiTheme::from_colors(&colors, AppearanceMode::Dark, &UiFontConfig::default());
        assert_eq!(theme.accent, to_color32(colors.normal[4]));
    }

    #[test]
    fn from_colors_warn_is_yellow() {
        let colors = dark_colors();
        let theme = UiTheme::from_colors(&colors, AppearanceMode::Dark, &UiFontConfig::default());
        assert_eq!(theme.warn, to_color32(colors.normal[3]));
    }

    #[test]
    fn from_colors_error_is_red() {
        let colors = dark_colors();
        let theme = UiTheme::from_colors(&colors, AppearanceMode::Dark, &UiFontConfig::default());
        assert_eq!(theme.error, to_color32(colors.normal[1]));
    }

    #[test]
    fn from_colors_default_metrics() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        assert_eq!(theme.rounding, 0);
        assert_eq!(theme.font_small, 12.0);
        assert_eq!(theme.font_list, 14.0);
        assert_eq!(theme.font_normal, 14.0);
        assert_eq!(theme.menu_width, 120.0);
    }

    // -- UiTheme::bg_with_alpha --

    #[test]
    fn bg_with_alpha_sets_alpha() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        let c = theme.bg_with_alpha(128);
        assert_eq!(c.a(), 128);
        // Full alpha should match bg exactly.
        let full = theme.bg_with_alpha(255);
        assert_eq!(full.r(), theme.bg.r());
        assert_eq!(full.g(), theme.bg.g());
        assert_eq!(full.b(), theme.bg.b());
    }

    #[test]
    fn bg_with_alpha_zero() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        let c = theme.bg_with_alpha(0);
        assert_eq!(c.a(), 0);
    }

    // -- UiTheme::to_visuals --

    #[test]
    fn to_visuals_dark_mode_flag() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        let v = theme.to_visuals();
        assert!(v.dark_mode);
    }

    #[test]
    fn to_visuals_all_corners_zero() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        let v = theme.to_visuals();
        assert_eq!(v.window_corner_radius, CornerRadius::ZERO);
        assert_eq!(v.menu_corner_radius, CornerRadius::ZERO);
    }

    #[test]
    fn to_visuals_expansion_values() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        let v = theme.to_visuals();
        assert_eq!(v.widgets.noninteractive.expansion, 0.0);
        assert_eq!(v.widgets.inactive.expansion, 0.0);
        // Flat style — no expansion on hover/active.
        assert_eq!(v.widgets.hovered.expansion, 0.0);
        assert_eq!(v.widgets.active.expansion, 0.0);
        assert_eq!(v.widgets.open.expansion, 0.0);
    }

    #[test]
    fn to_visuals_panel_fill_matches_bg() {
        let theme = UiTheme::from_colors(&dark_colors(), AppearanceMode::Dark, &UiFontConfig::default());
        let v = theme.to_visuals();
        assert_eq!(v.panel_fill, theme.bg);
    }
}
