//! Color conversion from alacritty_terminal's `Color` to RGBA `[f32; 4]`,
//! driven by a `ResolvedColors` struct built from a `ColorScheme`.

use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use conch_core::color_scheme::ColorScheme;

/// Pre-resolved RGBA colors for terminal rendering.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResolvedColors {
    pub background: [f32; 4],
    pub foreground: [f32; 4],
    /// Standard ANSI colors 0-7.
    pub normal: [[f32; 4]; 8],
    /// Bright ANSI colors 8-15.
    pub bright: [[f32; 4]; 8],
    /// Dim variants (auto-computed as 2/3 brightness if absent).
    pub dim: [[f32; 4]; 8],
    pub cursor_text: Option<[f32; 4]>,
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

        let cursor_text = scheme.cursor.as_ref().map(|c| hex_to_rgba(&c.text));
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
            cursor_text,
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
