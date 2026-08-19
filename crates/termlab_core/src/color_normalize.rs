//! Canonicalization of Alacritty color strings into a single `#rrggbb` form.
//!
//! ## Why this exists, and why it lives here
//!
//! [`crate::color_scheme::ColorScheme`] stores every color as a raw `String`
//! so that *every* form Alacritty accepts parses without error (see that
//! module's compatibility contract). But the consumers downstream do not want
//! four forms — `crates/termlab_tauri/src/theme.rs`'s `darken`/`lighten`/
//! `luminance`/`blend` helpers all do `trim_start_matches('#')` and then
//! hex-parse the first six characters, so a legacy `0x`-prefixed color like
//! `"0x1e1e2e"` gets sliced as `0x`/`1e`/`1e` and renders as `#001e1e`
//! instead of `#1e1e2e`. CSS custom properties and the xterm theme object are
//! equally literal.
//!
//! So the raw forms are canonicalized at exactly ONE boundary:
//! [`crate::color_scheme::load_theme`], the single fallible file →
//! `ColorScheme` funnel that `resolve_theme_in` (both its path branch and its
//! name branch) and `theme_list_entry_for` both go through.
//! `ColorScheme::default()` (the built-in TermLab Dark palette) is already
//! canonical, so after that one call site there is no way to obtain a
//! `ColorScheme` whose colors are not `#`-form. Every downstream consumer —
//! `theme.rs`'s helpers,
//! `PalettePreview`, the theme catalog — is fixed by that single call.
//!
//! ### Why not in a serde `Deserialize` impl?
//!
//! The `CellForeground`/`CellBackground` sentinels resolve against the
//! theme's own `primary.foreground`/`primary.background`. Those are *sibling*
//! fields, invisible to a field-level `Deserialize` impl. Normalizing the
//! whole `ColorScheme` after deserialization is the earliest point at which
//! that context exists. It also leaves the serde layer's verbatim fidelity
//! intact, so the parse-level round-trip tests keep pinning exactly what they
//! pinned.
//!
//! ### `CellRgb` semantics
//!
//! Alacritty resolves `CellForeground`/`CellBackground` per cell, at render
//! time, against the cell the cursor or selection is over. TermLab hands
//! xterm.js a static palette and has no per-cell context at all, so the
//! closest static equivalent is the theme's own default foreground /
//! background. That is not an arbitrary choice: Alacritty's own cursor
//! default is `text = CellBackground, cursor = CellForeground`, and under
//! this mapping a theme that spells that default out explicitly resolves to
//! `cursor_text = background, cursor_color = foreground` — byte-identical to
//! the fallback `theme.rs` already applies when a theme omits
//! `[colors.cursor]` entirely. The mapping agrees with shipped behavior
//! rather than inventing new behavior.
//!
//! ### What is deliberately NOT normalized
//!
//! Hex digit case is preserved. Several vendored themes (and both built-in
//! `TermLab *.toml` palettes) use uppercase hex, every consumer parses it
//! fine, and lowercasing would churn the pinned per-fixture assertions for no
//! functional gain. Canonical here means "starts with `#`, six hex digits",
//! not "lowercase".

/// The Alacritty sentinel meaning "the foreground of the cell underneath".
pub const CELL_FOREGROUND: &str = "CellForeground";
/// The Alacritty sentinel meaning "the background of the cell underneath".
pub const CELL_BACKGROUND: &str = "CellBackground";

/// Strip a `#`, `0x` or `0X` prefix and return the remaining body, or `None`
/// when the string carries no recognized prefix.
fn strip_color_prefix(value: &str) -> Option<&str> {
    if let Some(rest) = value.strip_prefix('#') {
        return Some(rest);
    }
    if let Some(rest) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return Some(rest);
    }
    None
}

/// Canonicalize one color string, without any cell context.
///
/// Handles the two hex forms Alacritty accepts (`#rrggbb` and the legacy
/// `0xrrggbb`), plus the 3-digit shorthand and an 8-digit `#rrggbbaa` that a
/// hand-written theme might carry. Anything unrecognized — including the
/// `CellRgb` sentinels, which [`normalize_color`] handles before calling
/// here — is returned verbatim, because guessing would be worse than leaving
/// the value for a human to see in a log.
fn normalize_hex_form(value: &str) -> String {
    let trimmed = value.trim();
    let Some(body) = strip_color_prefix(trimmed) else {
        return trimmed.to_string();
    };
    if !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return trimmed.to_string();
    }
    match body.len() {
        3 => {
            let mut out = String::with_capacity(7);
            out.push('#');
            for c in body.chars() {
                out.push(c);
                out.push(c);
            }
            out
        }
        6 | 8 => format!("#{body}"),
        _ => trimmed.to_string(),
    }
}

/// Canonicalize one color string to `#`-form, resolving the `CellRgb`
/// sentinels against the supplied theme foreground/background.
///
/// `foreground` and `background` must ALREADY be canonical (the caller
/// normalizes `primary` first) so that a sentinel never resolves to another
/// un-normalized form.
pub fn normalize_color(value: &str, foreground: &str, background: &str) -> String {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case(CELL_FOREGROUND) {
        return foreground.to_string();
    }
    if trimmed.eq_ignore_ascii_case(CELL_BACKGROUND) {
        return background.to_string();
    }
    normalize_hex_form(trimmed)
}

/// Canonicalize every color-bearing field of a parsed scheme in place.
///
/// `primary.background`/`primary.foreground` are normalized FIRST and then
/// used as the `CellRgb` context for everything else, so a sentinel can never
/// resolve to a still-raw `0x`-form value.
///
/// This is called from [`crate::color_scheme::load_theme`] and nowhere else —
/// see the module docs for why that is the single boundary.
pub fn normalize_scheme(scheme: &mut crate::color_scheme::ColorScheme) {
    // No cell context yet: a sentinel sitting in `primary` itself is
    // degenerate (it would refer to the very field being defined), so it is
    // left verbatim by passing the raw strings through normalize_hex_form.
    scheme.primary.background = normalize_hex_form(&scheme.primary.background);
    scheme.primary.foreground = normalize_hex_form(&scheme.primary.foreground);
    let bg = scheme.primary.background.clone();
    let fg = scheme.primary.foreground.clone();

    let fix = |value: &mut String| *value = normalize_color(value, &fg, &bg);

    if let Some(v) = scheme.primary.dim_foreground.as_mut() {
        fix(v);
    }
    if let Some(v) = scheme.primary.bright_foreground.as_mut() {
        fix(v);
    }

    for ansi in [
        Some(&mut scheme.normal),
        Some(&mut scheme.bright),
        scheme.dim.as_mut(),
    ]
    .into_iter()
    .flatten()
    {
        for v in [
            &mut ansi.black,
            &mut ansi.red,
            &mut ansi.green,
            &mut ansi.yellow,
            &mut ansi.blue,
            &mut ansi.magenta,
            &mut ansi.cyan,
            &mut ansi.white,
        ] {
            fix(v);
        }
    }

    for cursor in [scheme.cursor.as_mut(), scheme.vi_mode_cursor.as_mut()]
        .into_iter()
        .flatten()
    {
        fix(&mut cursor.text);
        fix(&mut cursor.cursor);
    }

    if let Some(selection) = scheme.selection.as_mut() {
        fix(&mut selection.text);
        fix(&mut selection.background);
    }

    if let Some(search) = scheme.search.as_mut() {
        for m in [search.matches.as_mut(), search.focused_match.as_mut()]
            .into_iter()
            .flatten()
        {
            fix(&mut m.foreground);
            fix(&mut m.background);
        }
    }

    if let Some(hints) = scheme.hints.as_mut() {
        for pair in [hints.start.as_mut(), hints.end.as_mut()]
            .into_iter()
            .flatten()
        {
            fix(&mut pair.foreground);
            fix(&mut pair.background);
        }
    }

    if let Some(line_indicator) = scheme.line_indicator.as_mut() {
        for v in [
            line_indicator.foreground.as_mut(),
            line_indicator.background.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            fix(v);
        }
    }

    if let Some(footer_bar) = scheme.footer_bar.as_mut() {
        for v in [
            footer_bar.foreground.as_mut(),
            footer_bar.background.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            fix(v);
        }
    }

    for indexed in scheme.indexed_colors.iter_mut() {
        fix(&mut indexed.color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FG: &str = "#f8f8f2";
    const BG: &str = "#282a36";

    #[test]
    fn hash_form_passes_through_unchanged() {
        assert_eq!(normalize_color("#1e1e2e", FG, BG), "#1e1e2e");
    }

    #[test]
    fn zero_x_form_becomes_hash_form() {
        assert_eq!(normalize_color("0x1e1e2e", FG, BG), "#1e1e2e");
        assert_eq!(normalize_color("0X1E1E2E", FG, BG), "#1E1E2E");
    }

    #[test]
    fn hex_digit_case_is_preserved() {
        // Several vendored fixtures and both built-in TermLab palettes use
        // uppercase hex; canonical means "#-prefixed", not "lowercase".
        assert_eq!(normalize_color("#D18616", FG, BG), "#D18616");
        assert_eq!(normalize_color("0xD18616", FG, BG), "#D18616");
    }

    #[test]
    fn three_digit_shorthand_expands() {
        assert_eq!(normalize_color("#fff", FG, BG), "#ffffff");
        assert_eq!(normalize_color("0xa1b", FG, BG), "#aa11bb");
    }

    #[test]
    fn eight_digit_rgba_keeps_all_eight_digits() {
        // theme.rs's helpers slice the first six characters after the '#',
        // so the RGB half still lands correctly.
        assert_eq!(normalize_color("0x1e1e2eff", FG, BG), "#1e1e2eff");
    }

    #[test]
    fn cell_foreground_resolves_to_the_theme_foreground() {
        assert_eq!(normalize_color("CellForeground", FG, BG), FG);
    }

    #[test]
    fn cell_background_resolves_to_the_theme_background() {
        assert_eq!(normalize_color("CellBackground", FG, BG), BG);
    }

    #[test]
    fn cell_sentinels_are_matched_case_insensitively() {
        assert_eq!(normalize_color("cellforeground", FG, BG), FG);
        assert_eq!(normalize_color("CELLBACKGROUND", FG, BG), BG);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(normalize_color("  0x1e1e2e  ", FG, BG), "#1e1e2e");
        assert_eq!(normalize_color(" CellForeground ", FG, BG), FG);
    }

    #[test]
    fn unrecognized_values_are_returned_verbatim() {
        // No guessing: an unknown form stays visible rather than silently
        // becoming some incidental color.
        assert_eq!(normalize_color("rebeccapurple", FG, BG), "rebeccapurple");
        assert_eq!(normalize_color("#nothex", FG, BG), "#nothex");
        assert_eq!(normalize_color("0x12345", FG, BG), "0x12345");
        assert_eq!(normalize_color("", FG, BG), "");
    }

    /// The concrete failure the Task-1 review found: `darken("0x1e1e2e", 0)`
    /// parses `"0x"`/`"1e"`/`"1e"` and yields `#001e1e`. Reproduced here
    /// against the same slicing rule so this module's contract is pinned to
    /// the bug it exists to prevent.
    #[test]
    fn zero_x_form_would_mis_slice_before_normalization() {
        fn first_six_channels(hex: &str) -> (i32, i32, i32) {
            let hex = hex.trim_start_matches('#');
            (
                i32::from_str_radix(&hex[0..2], 16).unwrap_or(0),
                i32::from_str_radix(&hex[2..4], 16).unwrap_or(0),
                i32::from_str_radix(&hex[4..6], 16).unwrap_or(0),
            )
        }
        assert_eq!(first_six_channels("0x1e1e2e"), (0, 0x1e, 0x1e));
        assert_eq!(
            first_six_channels(&normalize_color("0x1e1e2e", FG, BG)),
            (0x1e, 0x1e, 0x2e)
        );
    }
}
