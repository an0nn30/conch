//! Color theme loading — converts Alacritty .toml themes to CSS-compatible values.

use serde::Serialize;

use conch_core::config::UserConfig;

#[derive(Serialize)]
pub(crate) struct ThemeColors {
    pub background: String,
    pub foreground: String,
    pub cursor_text: String,
    pub cursor_color: String,
    pub selection_text: String,
    pub selection_bg: String,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
    pub dim_fg: String,
    pub panel_bg: String,
    pub tab_bar_bg: String,
    pub tab_border: String,
    pub input_bg: String,
    pub active_highlight: String,
    pub text_secondary: String,
    pub text_muted: String,
}

fn darken(hex: &str, amount: i32) -> String {
    let hex = hex.trim_start_matches('#');
    // Expand 3-char shorthand (#fff -> ffffff)
    let hex = if hex.len() == 3 {
        let b: Vec<u8> = hex.bytes().collect();
        format!(
            "{0}{0}{1}{1}{2}{2}",
            b[0] as char, b[1] as char, b[2] as char
        )
    } else if hex.len() < 6 {
        return format!("#{hex}");
    } else {
        hex.to_string()
    };
    let r = i32::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = i32::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = i32::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    format!(
        "#{:02x}{:02x}{:02x}",
        (r - amount).clamp(0, 255),
        (g - amount).clamp(0, 255),
        (b - amount).clamp(0, 255)
    )
}

fn lighten(hex: &str, amount: i32) -> String {
    darken(hex, -amount)
}

/// Compute relative luminance (0.0 = black, 1.0 = white) of a hex color.
fn luminance(hex: &str) -> f64 {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return 0.5;
    }
    let r = i32::from_str_radix(&hex[0..2], 16).unwrap_or(128) as f64 / 255.0;
    let g = i32::from_str_radix(&hex[2..4], 16).unwrap_or(128) as f64 / 255.0;
    let b = i32::from_str_radix(&hex[4..6], 16).unwrap_or(128) as f64 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Blend a color toward another by a fraction (0.0 = source, 1.0 = target).
fn blend(source: &str, target: &str, frac: f64) -> String {
    let s = source.trim_start_matches('#');
    let t = target.trim_start_matches('#');
    if s.len() < 6 || t.len() < 6 {
        return format!("#{s}");
    }
    let sr = i32::from_str_radix(&s[0..2], 16).unwrap_or(0) as f64;
    let sg = i32::from_str_radix(&s[2..4], 16).unwrap_or(0) as f64;
    let sb = i32::from_str_radix(&s[4..6], 16).unwrap_or(0) as f64;
    let tr = i32::from_str_radix(&t[0..2], 16).unwrap_or(0) as f64;
    let tg = i32::from_str_radix(&t[2..4], 16).unwrap_or(0) as f64;
    let tb = i32::from_str_radix(&t[4..6], 16).unwrap_or(0) as f64;
    format!(
        "#{:02x}{:02x}{:02x}",
        (sr + (tr - sr) * frac).round().clamp(0.0, 255.0) as u8,
        (sg + (tg - sg) * frac).round().clamp(0.0, 255.0) as u8,
        (sb + (tb - sb) * frac).round().clamp(0.0, 255.0) as u8
    )
}

/// Resolve theme colors from a pre-loaded ColorScheme (no config needed).
pub(crate) fn resolve_theme_colors_from_scheme(
    scheme: &conch_core::color_scheme::ColorScheme,
) -> ThemeColors {
    let bg = &scheme.primary.background;
    let fg = &scheme.primary.foreground;
    let cursor = scheme.cursor.as_ref();
    let selection = scheme.selection.as_ref();

    ThemeColors {
        background: bg.clone(),
        foreground: fg.clone(),
        cursor_text: cursor.map(|c| c.text.clone()).unwrap_or_else(|| bg.clone()),
        cursor_color: cursor
            .map(|c| c.cursor.clone())
            .unwrap_or_else(|| fg.clone()),
        selection_text: selection
            .map(|s| s.text.clone())
            .unwrap_or_else(|| fg.clone()),
        selection_bg: selection
            .map(|s| s.background.clone())
            .unwrap_or_else(|| lighten(bg, 30)),
        black: scheme.normal.black.clone(),
        red: scheme.normal.red.clone(),
        green: scheme.normal.green.clone(),
        yellow: scheme.normal.yellow.clone(),
        blue: scheme.normal.blue.clone(),
        magenta: scheme.normal.magenta.clone(),
        cyan: scheme.normal.cyan.clone(),
        white: scheme.normal.white.clone(),
        bright_black: scheme.bright.black.clone(),
        bright_red: scheme.bright.red.clone(),
        bright_green: scheme.bright.green.clone(),
        bright_yellow: scheme.bright.yellow.clone(),
        bright_blue: scheme.bright.blue.clone(),
        bright_magenta: scheme.bright.magenta.clone(),
        bright_cyan: scheme.bright.cyan.clone(),
        bright_white: scheme.bright.white.clone(),
        // Detect dark vs light theme: dark bg = lighten toward white, light bg = darken toward black.
        dim_fg: scheme
            .primary
            .dim_foreground
            .clone()
            .unwrap_or_else(|| blend(fg, bg, 0.50)),
        panel_bg: if luminance(bg) < 0.5 {
            darken(bg, 8)
        } else {
            lighten(bg, 8)
        },
        tab_bar_bg: if luminance(bg) < 0.5 {
            darken(bg, 14)
        } else {
            lighten(bg, 14)
        },
        tab_border: if luminance(bg) < 0.5 {
            lighten(bg, 18)
        } else {
            darken(bg, 18)
        },
        input_bg: if luminance(bg) < 0.5 {
            lighten(bg, 10)
        } else {
            darken(bg, 10)
        },
        active_highlight: if luminance(bg) < 0.5 {
            lighten(bg, 28)
        } else {
            darken(bg, 28)
        },
        // Derive text colors by blending fg toward bg for reduced emphasis.
        text_secondary: blend(fg, bg, 0.25),
        text_muted: blend(fg, bg, 0.50),
    }
}

pub(crate) fn resolve_theme_colors(config: &UserConfig) -> ThemeColors {
    let scheme = conch_core::color_scheme::resolve_theme(&config.colors.theme);
    resolve_theme_colors_from_scheme(&scheme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conch_core::color_scheme::ColorScheme;

    #[test]
    fn resolve_from_scheme_uses_primary_colors() {
        let scheme = ColorScheme::default(); // Dracula
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_eq!(tc.background, "#282a36");
        assert_eq!(tc.foreground, "#f8f8f2");
    }

    #[test]
    fn resolve_from_scheme_derives_panel_colors() {
        let scheme = ColorScheme::default();
        let tc = resolve_theme_colors_from_scheme(&scheme);
        // panel_bg should be darker than background
        assert_ne!(tc.panel_bg, tc.background);
        // tab_bar_bg should be darker than panel_bg
        assert_ne!(tc.tab_bar_bg, tc.panel_bg);
        // input_bg should be lighter than background
        assert_ne!(tc.input_bg, tc.background);
    }

    #[test]
    fn resolve_from_scheme_maps_ansi_colors() {
        let scheme = ColorScheme::default();
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_eq!(tc.red, "#ff5555");
        assert_eq!(tc.green, "#50fa7b");
        assert_eq!(tc.bright_red, "#ff6e6e");
        assert_eq!(tc.bright_green, "#69ff94");
    }

    #[test]
    fn resolve_from_scheme_handles_cursor_colors() {
        let scheme = ColorScheme::default(); // has cursor colors
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_eq!(tc.cursor_text, "#282a36");
        assert_eq!(tc.cursor_color, "#f8f8f2");
    }

    #[test]
    fn resolve_from_scheme_fallback_when_no_cursor() {
        let mut scheme = ColorScheme::default();
        scheme.cursor = None;
        let tc = resolve_theme_colors_from_scheme(&scheme);
        // Falls back to bg/fg
        assert_eq!(tc.cursor_text, scheme.primary.background);
        assert_eq!(tc.cursor_color, scheme.primary.foreground);
    }

    #[test]
    fn darken_expands_three_char_hex() {
        // #fff should expand to #ffffff then darken by 10
        let result = darken("#fff", 10);
        assert_eq!(result, "#f5f5f5");
    }

    #[test]
    fn lighten_expands_three_char_hex() {
        let result = lighten("#000", 10);
        assert_eq!(result, "#0a0a0a");
    }

    #[test]
    fn blend_midpoint() {
        // Blend black toward white at 50% = #808080 (gray)
        let result = blend("#000000", "#ffffff", 0.5);
        assert_eq!(result, "#808080");
    }

    #[test]
    fn blend_zero_returns_source() {
        let result = blend("#ff0000", "#0000ff", 0.0);
        assert_eq!(result, "#ff0000");
    }

    #[test]
    fn blend_one_returns_target() {
        let result = blend("#ff0000", "#0000ff", 1.0);
        assert_eq!(result, "#0000ff");
    }

    #[test]
    fn luminance_black_is_zero() {
        assert!((luminance("#000000") - 0.0).abs() < 0.01);
    }

    #[test]
    fn luminance_white_is_one() {
        assert!((luminance("#ffffff") - 1.0).abs() < 0.01);
    }

    #[test]
    fn text_secondary_differs_from_fg() {
        let scheme = ColorScheme::default(); // Dracula: light fg on dark bg
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_ne!(tc.text_secondary, tc.foreground);
        assert_ne!(tc.text_secondary, tc.background);
    }

    #[test]
    fn text_muted_more_blended_than_secondary() {
        let scheme = ColorScheme::default();
        let tc = resolve_theme_colors_from_scheme(&scheme);
        // text_muted should be closer to bg than text_secondary
        assert_ne!(tc.text_muted, tc.text_secondary);
    }
}
