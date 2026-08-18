//! Native cell metrics for the bundled terminal font.
//!
//! The webview renders the terminal with a vendored JetBrains Mono loaded via
//! `@font-face`, so the cell size only truly exists inside the webview — which
//! is why the window used to open at a guess and visibly correct itself. This
//! module is Alacritty's answer adapted to that constraint: read the metrics
//! straight out of the font file, before any window exists, and open at the
//! right size on the first frame.
//!
//! Why `ttf-parser` over `crossfont`: crossfont resolves fonts by SYSTEM name
//! (Alacritty makes users install their fonts), but our default font ships
//! inside the app and is not installed — a system lookup for "JetBrains Mono"
//! would silently measure whatever fallback the OS picked. Parsing the file we
//! actually ship measures the bytes the webview actually renders. The `.ttf`
//! here and the `.woff2` in `frontend/vendor/fonts/` are the same upstream
//! release (v2.304) — same tables, different compression. Keep them in step
//! when either is upgraded.

/// Same bytes as `frontend/vendor/fonts/JetBrainsMono-Regular.woff2`,
/// uncompressed, because ttf-parser does not read woff2.
const DEFAULT_FONT: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");

/// The family the config resolves to when the user has not chosen a font —
/// must match `FontFamily::default()` in termlab_core.
const DEFAULT_FAMILY: &str = "JetBrains Mono";

/// Cell size in logical pixels for the bundled font at `px` font-size, or
/// `None` when the configured family is not the bundled one (a user-chosen
/// system font we cannot measure from here — the persisted webview
/// measurement covers that case from its second launch on).
///
/// Height replicates how WebKit computes a "normal" line box on macOS: ascent,
/// descent and line gap are each scaled then ceiled SEPARATELY before summing.
/// Summing first and ceiling once gives 18.48 -> 19 for JetBrains Mono at
/// 14px, while WebKit produces 20 — a whole row of drift across a 50-row
/// window. If a launch still opens a hair off, suspect this rounding first.
pub(crate) fn default_font_cell(family: &str, px: f64) -> Option<(f64, f64)> {
    if px <= 0.0 || !family_is_default(family) {
        return None;
    }
    let face = ttf_parser::Face::parse(DEFAULT_FONT, 0).ok()?;
    let upm = face.units_per_em() as f64;
    if upm <= 0.0 {
        return None;
    }

    // Every glyph in a monospace font advances the same; 'W' by convention
    // (it is also what xterm.js measures).
    let glyph = face.glyph_index('W')?;
    let advance = face.glyph_hor_advance(glyph)? as f64;
    let cell_w = advance * px / upm;

    let ascent = (face.ascender() as f64 * px / upm).ceil();
    let descent = (-(face.descender() as f64) * px / upm).ceil();
    let gap = (face.line_gap() as f64 * px / upm).ceil();
    let cell_h = ascent + descent + gap;

    Some((cell_w, cell_h))
}

/// True for the bundled family, alone or leading a fallback stack.
fn family_is_default(family: &str) -> bool {
    let head = family.split(',').next().unwrap_or("").trim();
    let head = head.trim_matches(|c| c == '"' || c == '\'');
    head.is_empty() || head.eq_ignore_ascii_case(DEFAULT_FAMILY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetbrains_mono_at_14px_matches_the_font_tables() {
        // Pinned against the vendored v2.304 file: upm 1000, 'W' advance 600,
        // hhea ascender 1020, descender -300, line gap 0.
        let (w, h) = default_font_cell(DEFAULT_FAMILY, 14.0).unwrap();
        assert!((w - 8.4).abs() < 1e-9, "cell width was {w}");
        // ceil(14.28) + ceil(4.2) + 0 = 15 + 5
        assert!((h - 20.0).abs() < 1e-9, "cell height was {h}");
    }

    #[test]
    fn scales_linearly_in_width_but_ceils_height_components() {
        let (w14, _) = default_font_cell(DEFAULT_FAMILY, 14.0).unwrap();
        let (w28, h28) = default_font_cell(DEFAULT_FAMILY, 28.0).unwrap();
        assert!((w28 - 2.0 * w14).abs() < 1e-9);
        // ceil(28.56) + ceil(8.4) = 29 + 9
        assert!((h28 - 38.0).abs() < 1e-9, "cell height at 28 was {h28}");
    }

    #[test]
    fn fallback_stacks_and_quotes_still_count_as_default() {
        assert!(default_font_cell("\"JetBrains Mono\", Menlo, monospace", 14.0).is_some());
        assert!(default_font_cell("jetbrains mono", 14.0).is_some());
        assert!(default_font_cell("", 14.0).is_some());
    }

    #[test]
    fn a_user_chosen_family_is_not_guessed_at() {
        assert_eq!(default_font_cell("Menlo", 14.0), None);
        assert_eq!(default_font_cell("Fira Code, JetBrains Mono", 14.0), None);
    }

    #[test]
    fn nonsense_size_is_refused() {
        assert_eq!(default_font_cell(DEFAULT_FAMILY, 0.0), None);
        assert_eq!(default_font_cell(DEFAULT_FAMILY, -14.0), None);
    }
}
