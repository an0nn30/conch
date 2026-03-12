//! Color conversion from alacritty_terminal's `Color` to RGBA `[f32; 4]`,
//! driven by a `ResolvedColors` struct built from a `ColorScheme`.

use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use conch_core::color_scheme::ColorScheme;

/// Pre-resolved RGBA colors for terminal rendering.
#[derive(Debug, Clone)]
pub struct ResolvedColors {
    pub background: [f32; 4],
    pub foreground: [f32; 4],
    /// Standard ANSI colors 0-7.
    pub normal: [[f32; 4]; 8],
    /// Bright ANSI colors 8-15.
    pub bright: [[f32; 4]; 8],
    /// Dim variants (auto-computed as 2/3 brightness if absent).
    pub dim: [[f32; 4]; 8],
    pub cursor_color: Option<[f32; 4]>,
    pub selection_text: Option<[f32; 4]>,
    pub selection_bg: Option<[f32; 4]>,
    pub bright_foreground: Option<[f32; 4]>,
    pub dim_foreground: Option<[f32; 4]>,
}

impl ResolvedColors {
    /// Build resolved colors from a color scheme.
    pub fn from_scheme(scheme: &ColorScheme) -> Self {
        let background = hex_to_rgba(&scheme.primary.background);
        let foreground = hex_to_rgba(&scheme.primary.foreground);

        let normal = resolve_ansi(&scheme.normal);
        let bright = resolve_ansi(&scheme.bright);

        let dim = if let Some(dim_ansi) = &scheme.dim {
            resolve_ansi(dim_ansi)
        } else {
            // Auto-compute dim as 2/3 brightness of normal.
            let mut d = [[0.0f32; 4]; 8];
            for (i, c) in normal.iter().enumerate() {
                d[i] = [c[0] * 0.67, c[1] * 0.67, c[2] * 0.67, 1.0];
            }
            d
        };

        let cursor_color = scheme.cursor.as_ref().map(|c| hex_to_rgba(&c.cursor));
        let selection_text = scheme.selection.as_ref().map(|s| hex_to_rgba(&s.text));
        let selection_bg = scheme.selection.as_ref().map(|s| hex_to_rgba(&s.background));
        let bright_foreground = scheme.primary.bright_foreground.as_ref().map(|s| hex_to_rgba(s));
        let dim_foreground = scheme.primary.dim_foreground.as_ref().map(|s| hex_to_rgba(s));

        Self {
            background,
            foreground,
            normal,
            bright,
            dim,
            cursor_color,
            selection_text,
            selection_bg,
            bright_foreground,
            dim_foreground,
        }
    }
}

/// Convert an alacritty terminal color to RGBA using resolved scheme colors.
pub fn convert_color(color: TermColor, colors: &ResolvedColors) -> [f32; 4] {
    match color {
        TermColor::Spec(rgb) => [
            rgb.r as f32 / 255.0,
            rgb.g as f32 / 255.0,
            rgb.b as f32 / 255.0,
            1.0,
        ],
        TermColor::Indexed(idx) => indexed_color_to_rgba(idx, colors),
        TermColor::Named(named) => named_color_to_rgba(named, colors),
    }
}

/// Map named ANSI colors to resolved scheme values.
fn named_color_to_rgba(c: NamedColor, colors: &ResolvedColors) -> [f32; 4] {
    match c {
        NamedColor::Black => colors.normal[0],
        NamedColor::Red => colors.normal[1],
        NamedColor::Green => colors.normal[2],
        NamedColor::Yellow => colors.normal[3],
        NamedColor::Blue => colors.normal[4],
        NamedColor::Magenta => colors.normal[5],
        NamedColor::Cyan => colors.normal[6],
        NamedColor::White => colors.normal[7],

        NamedColor::BrightBlack => colors.bright[0],
        NamedColor::BrightRed => colors.bright[1],
        NamedColor::BrightGreen => colors.bright[2],
        NamedColor::BrightYellow => colors.bright[3],
        NamedColor::BrightBlue => colors.bright[4],
        NamedColor::BrightMagenta => colors.bright[5],
        NamedColor::BrightCyan => colors.bright[6],
        NamedColor::BrightWhite => colors.bright[7],

        NamedColor::DimBlack => colors.dim[0],
        NamedColor::DimRed => colors.dim[1],
        NamedColor::DimGreen => colors.dim[2],
        NamedColor::DimYellow => colors.dim[3],
        NamedColor::DimBlue => colors.dim[4],
        NamedColor::DimMagenta => colors.dim[5],
        NamedColor::DimCyan => colors.dim[6],
        NamedColor::DimWhite => colors.dim[7],

        NamedColor::Foreground => colors.foreground,
        NamedColor::Background => colors.background,
        NamedColor::Cursor => colors.cursor_color.unwrap_or(colors.foreground),
        NamedColor::BrightForeground => colors.bright_foreground.unwrap_or(colors.foreground),
        NamedColor::DimForeground => colors.dim_foreground.unwrap_or(colors.foreground),
    }
}

/// Convert a 256-color index to RGBA using resolved scheme colors.
fn indexed_color_to_rgba(idx: u8, colors: &ResolvedColors) -> [f32; 4] {
    if idx < 8 {
        colors.normal[idx as usize]
    } else if idx < 16 {
        colors.bright[(idx - 8) as usize]
    } else if idx < 232 {
        // 6x6x6 color cube.
        let i = idx - 16;
        let r = (i / 36) as f32 / 5.0;
        let g = ((i / 6) % 6) as f32 / 5.0;
        let b = (i % 6) as f32 / 5.0;
        [r, g, b, 1.0]
    } else {
        // 24-step grayscale ramp.
        let gray = (idx - 232) as f32 / 23.0;
        [gray, gray, gray, 1.0]
    }
}

/// Resolve an `AnsiColors` to 8 RGBA values.
fn resolve_ansi(ansi: &conch_core::color_scheme::AnsiColors) -> [[f32; 4]; 8] {
    let arr = ansi.as_array();
    let mut out = [[0.0f32; 4]; 8];
    for (i, hex) in arr.iter().enumerate() {
        out[i] = hex_to_rgba(hex);
    }
    out
}

/// Parse a hex color string (`#RRGGBB` or `0xRRGGBB`) to `[f32; 4]` RGBA.
pub fn hex_to_rgba(hex: &str) -> [f32; 4] {
    let hex = hex.trim_start_matches('#').trim_start_matches("0x");
    if hex.len() < 6 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f32 / 255.0;
    [r, g, b, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: approximate float comparison for RGBA values.
    fn approx_eq(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 0.005)
    }

    // -- hex_to_rgba --

    #[test]
    fn hex_to_rgba_basic_white() {
        assert!(approx_eq(hex_to_rgba("#FFFFFF"), [1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_basic_black() {
        assert!(approx_eq(hex_to_rgba("#000000"), [0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_red() {
        assert!(approx_eq(hex_to_rgba("#FF0000"), [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_green() {
        assert!(approx_eq(hex_to_rgba("#00FF00"), [0.0, 1.0, 0.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_blue() {
        assert!(approx_eq(hex_to_rgba("#0000FF"), [0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_dracula_background() {
        // Dracula background: #282a36
        let c = hex_to_rgba("#282a36");
        assert!(approx_eq(c, [0x28 as f32 / 255.0, 0x2a as f32 / 255.0, 0x36 as f32 / 255.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_0x_prefix() {
        assert!(approx_eq(hex_to_rgba("0xFF0000"), [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_no_prefix() {
        assert!(approx_eq(hex_to_rgba("FF0000"), [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_lowercase() {
        assert!(approx_eq(hex_to_rgba("#ff00ff"), [1.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn hex_to_rgba_short_string_returns_black() {
        assert_eq!(hex_to_rgba("#FFF"), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn hex_to_rgba_empty_returns_black() {
        assert_eq!(hex_to_rgba(""), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn hex_to_rgba_invalid_hex_digits() {
        // "ZZZZZZ" — from_str_radix returns 0 for each component.
        assert_eq!(hex_to_rgba("#ZZZZZZ"), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn hex_to_rgba_alpha_is_always_1() {
        let c = hex_to_rgba("#808080");
        assert_eq!(c[3], 1.0);
    }

    // -- indexed_color_to_rgba --

    fn test_colors() -> ResolvedColors {
        let normal = [
            [0.0, 0.0, 0.0, 1.0], // 0 black
            [1.0, 0.0, 0.0, 1.0], // 1 red
            [0.0, 1.0, 0.0, 1.0], // 2 green
            [1.0, 1.0, 0.0, 1.0], // 3 yellow
            [0.0, 0.0, 1.0, 1.0], // 4 blue
            [1.0, 0.0, 1.0, 1.0], // 5 magenta
            [0.0, 1.0, 1.0, 1.0], // 6 cyan
            [1.0, 1.0, 1.0, 1.0], // 7 white
        ];
        let bright = [
            [0.5, 0.5, 0.5, 1.0], // 8
            [1.0, 0.5, 0.5, 1.0], // 9
            [0.5, 1.0, 0.5, 1.0], // 10
            [1.0, 1.0, 0.5, 1.0], // 11
            [0.5, 0.5, 1.0, 1.0], // 12
            [1.0, 0.5, 1.0, 1.0], // 13
            [0.5, 1.0, 1.0, 1.0], // 14
            [1.0, 1.0, 1.0, 1.0], // 15
        ];
        ResolvedColors {
            background: [0.0, 0.0, 0.0, 1.0],
            foreground: [1.0, 1.0, 1.0, 1.0],
            normal,
            bright,
            dim: normal, // Reuse for simplicity.
            cursor_color: None,
            selection_text: None,
            selection_bg: None,
            bright_foreground: None,
            dim_foreground: None,
        }
    }

    #[test]
    fn indexed_normal_range() {
        let colors = test_colors();
        assert_eq!(indexed_color_to_rgba(0, &colors), colors.normal[0]);
        assert_eq!(indexed_color_to_rgba(7, &colors), colors.normal[7]);
    }

    #[test]
    fn indexed_bright_range() {
        let colors = test_colors();
        assert_eq!(indexed_color_to_rgba(8, &colors), colors.bright[0]);
        assert_eq!(indexed_color_to_rgba(15, &colors), colors.bright[7]);
    }

    #[test]
    fn indexed_color_cube_origin() {
        let colors = test_colors();
        // Index 16 = (0, 0, 0) in the 6x6x6 cube.
        assert!(approx_eq(indexed_color_to_rgba(16, &colors), [0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn indexed_color_cube_max() {
        let colors = test_colors();
        // Index 231 = (5, 5, 5) → all 1.0.
        assert!(approx_eq(indexed_color_to_rgba(231, &colors), [1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn indexed_color_cube_pure_red() {
        let colors = test_colors();
        // Index 196 = (5, 0, 0): i = 196-16 = 180; r=180/36=5, g=0, b=0.
        assert!(approx_eq(indexed_color_to_rgba(196, &colors), [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn indexed_grayscale_darkest() {
        let colors = test_colors();
        // Index 232 = first grayscale step = 0/23 ≈ 0.0.
        let c = indexed_color_to_rgba(232, &colors);
        assert!(c[0] < 0.01);
    }

    #[test]
    fn indexed_grayscale_lightest() {
        let colors = test_colors();
        // Index 255 = last grayscale = 23/23 = 1.0.
        assert!(approx_eq(indexed_color_to_rgba(255, &colors), [1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn indexed_grayscale_midpoint() {
        let colors = test_colors();
        // Index 244 = 12/23 ≈ 0.522.
        let c = indexed_color_to_rgba(244, &colors);
        assert!((c[0] - 12.0 / 23.0).abs() < 0.005);
        assert_eq!(c[0], c[1]); // Gray: all channels equal.
        assert_eq!(c[1], c[2]);
    }

    // -- named_color_to_rgba --

    #[test]
    fn named_foreground() {
        let colors = test_colors();
        assert_eq!(named_color_to_rgba(NamedColor::Foreground, &colors), colors.foreground);
    }

    #[test]
    fn named_background() {
        let colors = test_colors();
        assert_eq!(named_color_to_rgba(NamedColor::Background, &colors), colors.background);
    }

    #[test]
    fn named_cursor_falls_back_to_foreground() {
        let colors = test_colors(); // cursor_color is None.
        assert_eq!(named_color_to_rgba(NamedColor::Cursor, &colors), colors.foreground);
    }

    #[test]
    fn named_cursor_uses_explicit_color() {
        let mut colors = test_colors();
        colors.cursor_color = Some([0.5, 0.5, 0.5, 1.0]);
        assert_eq!(named_color_to_rgba(NamedColor::Cursor, &colors), [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn named_normal_colors_map_correctly() {
        let colors = test_colors();
        assert_eq!(named_color_to_rgba(NamedColor::Red, &colors), colors.normal[1]);
        assert_eq!(named_color_to_rgba(NamedColor::Blue, &colors), colors.normal[4]);
    }

    #[test]
    fn named_bright_colors_map_correctly() {
        let colors = test_colors();
        assert_eq!(named_color_to_rgba(NamedColor::BrightRed, &colors), colors.bright[1]);
    }

    // -- convert_color (integration of above) --

    #[test]
    fn convert_spec_color() {
        use alacritty_terminal::vte::ansi::Rgb;
        let colors = test_colors();
        let c = convert_color(TermColor::Spec(Rgb { r: 128, g: 64, b: 255 }), &colors);
        assert!(approx_eq(c, [128.0 / 255.0, 64.0 / 255.0, 1.0, 1.0]));
    }

    #[test]
    fn convert_indexed_color() {
        let colors = test_colors();
        let c = convert_color(TermColor::Indexed(0), &colors);
        assert_eq!(c, colors.normal[0]);
    }

    #[test]
    fn convert_named_color() {
        let colors = test_colors();
        let c = convert_color(TermColor::Named(NamedColor::Green), &colors);
        assert_eq!(c, colors.normal[2]);
    }
}
