//! Color theme loading — converts Alacritty .toml themes to CSS-compatible values.

use serde::Serialize;
use ts_rs::TS;

use termlab_core::config::UserConfig;

#[derive(Serialize, TS)]
#[ts(export)]
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
    /// xterm.js `ITheme.extendedAnsi`: ANSI slots 16.. , element 0 == slot 16.
    /// `None` means "this theme does not override that slot"; the frontend
    /// turns those into `undefined`, which xterm leaves at its own default.
    /// Empty when the theme carries no `indexed_colors` — the frontend then
    /// omits the key entirely, so a theme without indexed colors produces the
    /// exact xterm theme object it produced before this field existed.
    /// See `crate::extended_ansi` for the hole-tolerance trace.
    pub extended_ansi: Vec<Option<String>>,
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
    scheme: &termlab_core::color_scheme::ColorScheme,
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
        extended_ansi: crate::extended_ansi::build_extended_ansi(&scheme.indexed_colors),
    }
}

/// Resolve the theme colors for a given RESOLVED app appearance
/// ('dark' | 'light'), honoring the reserved `auto` theme name.
///
/// `None` means the caller could not resolve an appearance and is treated as
/// dark — the same unresolvable-is-dark convention used by `appearance.js`,
/// `resolveNativeWindowTheme`, and `AppearanceMode::resolved_hint`. This is
/// also the back-compat path for a `get_theme_colors` invoke that predates
/// the argument.
pub(crate) fn resolve_theme_colors_for_appearance(
    config: &UserConfig,
    resolved_appearance: Option<&str>,
) -> ThemeColors {
    let scheme = termlab_core::effective_theme::resolve_effective_theme(
        &config.colors,
        resolved_appearance.unwrap_or(termlab_core::effective_theme::DEFAULT_RESOLVED_APPEARANCE),
    );
    resolve_theme_colors_from_scheme(&scheme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termlab_core::color_scheme::ColorScheme;

    #[test]
    fn resolve_from_scheme_uses_primary_colors() {
        let scheme = ColorScheme::default(); // TermLab Dark
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
        let scheme = ColorScheme::default(); // TermLab Dark: light fg on dark bg
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

    // ---------------------------------------------------------------------
    // Color-form normalization, end to end
    //
    // These go through load_theme -> resolve_theme_colors_from_scheme, i.e.
    // the real pipeline including the darken/lighten/luminance/blend helpers
    // above. That is the gap the Task-1 review found: the serde-level tests
    // in color_scheme.rs pin that the forms PARSE, and these pin that they
    // RENDER correctly.
    // ---------------------------------------------------------------------

    /// Write a theme file into a fresh temp dir and load it through the real
    /// `load_theme` boundary (which is where normalization happens).
    fn load_theme_str(label: &str, toml_str: &str) -> ColorScheme {
        let dir = std::env::temp_dir().join(format!(
            "termlab-theme-test-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("theme.toml");
        std::fs::write(&path, toml_str).expect("write theme");
        let scheme = termlab_core::color_scheme::load_theme(&path).expect("test theme must parse");
        let _ = std::fs::remove_dir_all(&dir);
        scheme
    }

    /// Body of a theme whose colors are all written in the given form.
    /// `{p}` is the prefix (`#` or `0x`).
    fn theme_in_form(prefix: &str) -> String {
        format!(
            r##"
[colors.primary]
background = "{prefix}1e1e2e"
foreground = "{prefix}cdd6f4"
dim_foreground = "{prefix}7f849c"

[colors.cursor]
text = "{prefix}1e1e2e"
cursor = "{prefix}f5e0dc"

[colors.selection]
text = "{prefix}1e1e2e"
background = "{prefix}f5e0dc"

[colors.normal]
black   = "{prefix}45475a"
red     = "{prefix}f38ba8"
green   = "{prefix}a6e3a1"
yellow  = "{prefix}f9e2af"
blue    = "{prefix}89b4fa"
magenta = "{prefix}f5c2e7"
cyan    = "{prefix}94e2d5"
white   = "{prefix}bac2de"

[colors.bright]
black   = "{prefix}585b70"
red     = "{prefix}f38ba8"
green   = "{prefix}a6e3a1"
yellow  = "{prefix}f9e2af"
blue    = "{prefix}89b4fa"
magenta = "{prefix}f5c2e7"
cyan    = "{prefix}94e2d5"
white   = "{prefix}a6adc8"
"##
        )
    }

    /// The headline regression: a `0x`-form theme must resolve to exactly the
    /// same rendered colors as the identical theme written in `#` form —
    /// including the DERIVED fields, which is where the mis-slicing showed
    /// up (`darken("0x1e1e2e", 0)` used to yield `#001e1e`).
    #[test]
    fn a_0x_form_theme_renders_identically_to_the_same_theme_in_hash_form() {
        let hash = resolve_theme_colors_from_scheme(&load_theme_str("hash", &theme_in_form("#")));
        let zero_x = resolve_theme_colors_from_scheme(&load_theme_str("0x", &theme_in_form("0x")));

        assert_eq!(zero_x.background, "#1e1e2e");
        assert_eq!(zero_x.foreground, "#cdd6f4");
        assert_eq!(zero_x.red, "#f38ba8");
        assert_eq!(zero_x.bright_white, "#a6adc8");
        assert_eq!(zero_x.cursor_color, "#f5e0dc");
        assert_eq!(zero_x.selection_bg, "#f5e0dc");
        assert_eq!(zero_x.dim_fg, "#7f849c");

        // The derived fields: these are the ones that silently went wrong.
        assert_eq!(zero_x.panel_bg, hash.panel_bg);
        assert_eq!(zero_x.tab_bar_bg, hash.tab_bar_bg);
        assert_eq!(zero_x.tab_border, hash.tab_border);
        assert_eq!(zero_x.input_bg, hash.input_bg);
        assert_eq!(zero_x.active_highlight, hash.active_highlight);
        assert_eq!(zero_x.text_secondary, hash.text_secondary);
        assert_eq!(zero_x.text_muted, hash.text_muted);

        // And a positive statement of what "correct" is: a dark background
        // darkens toward black for the panel, so panel_bg is the background
        // minus 8 per channel.
        assert_eq!(zero_x.panel_bg, "#161626");
    }

    /// `CellForeground`/`CellBackground` resolve to the theme's own fg/bg.
    /// The cursor case doubles as the justification check: a theme spelling
    /// out Alacritty's own cursor default must land on exactly what
    /// `resolve_from_scheme_fallback_when_no_cursor` produces for a theme
    /// with no `[colors.cursor]` at all.
    #[test]
    fn cell_rgb_sentinels_resolve_to_the_themes_own_foreground_and_background() {
        let toml_str = r##"
[colors.primary]
background = "#1f1f1f"
foreground = "#e3e3e3"

[colors.cursor]
text = "CellBackground"
cursor = "CellForeground"

[colors.selection]
text = "CellBackground"
background = "CellForeground"

[colors.normal]
black = "#000000"
red = "#b21818"
green = "#18b218"
yellow = "#b26818"
blue = "#1818b2"
magenta = "#b218b2"
cyan = "#18b2b2"
white = "#b2b2b2"

[colors.bright]
black = "#686868"
red = "#ff5454"
green = "#54ff54"
yellow = "#ffff54"
blue = "#5454ff"
magenta = "#ff54ff"
cyan = "#54ffff"
white = "#ffffff"
"##;
        let tc = resolve_theme_colors_from_scheme(&load_theme_str("cellrgb", toml_str));

        assert_eq!(tc.cursor_text, "#1f1f1f", "CellBackground -> primary bg");
        assert_eq!(tc.cursor_color, "#e3e3e3", "CellForeground -> primary fg");
        assert_eq!(tc.selection_text, "#1f1f1f");
        assert_eq!(tc.selection_bg, "#e3e3e3");

        // The agreement check: Alacritty's cursor default is
        // `text = CellBackground, cursor = CellForeground`, and theme.rs
        // already falls back to bg/fg when a theme omits [colors.cursor].
        // Spelling the default out must therefore change nothing.
        let mut without_cursor = load_theme_str("cellrgb-nocursor", toml_str);
        without_cursor.cursor = None;
        let fallback = resolve_theme_colors_from_scheme(&without_cursor);
        assert_eq!(tc.cursor_text, fallback.cursor_text);
        assert_eq!(tc.cursor_color, fallback.cursor_color);

        // No sentinel string survives into the rendered payload.
        for value in [
            &tc.cursor_text,
            &tc.cursor_color,
            &tc.selection_text,
            &tc.selection_bg,
            &tc.panel_bg,
            &tc.text_muted,
        ] {
            assert!(
                value.starts_with('#'),
                "{value} is not canonical #-form after normalization"
            );
        }
    }

    // ---------------------------------------------------------------------
    // indexed_colors -> xterm ITheme.extendedAnsi
    // ---------------------------------------------------------------------

    #[test]
    fn a_theme_without_indexed_colors_carries_an_empty_extended_ansi() {
        let tc = resolve_theme_colors_from_scheme(&ColorScheme::default());
        assert!(
            tc.extended_ansi.is_empty(),
            "no indexed_colors must mean no extendedAnsi key at all"
        );
    }

    /// The github_dark fixture's shape, asserted as the exact expected array.
    #[test]
    fn indexed_colors_become_the_extended_ansi_array() {
        let scheme = ColorScheme {
            indexed_colors: vec![
                termlab_core::color_scheme::IndexedColor {
                    index: 16,
                    color: "#d18616".into(),
                },
                termlab_core::color_scheme::IndexedColor {
                    index: 18,
                    color: "#f97583".into(),
                },
            ],
            ..ColorScheme::default()
        };
        let tc = resolve_theme_colors_from_scheme(&scheme);
        assert_eq!(
            tc.extended_ansi,
            vec![
                Some("#d18616".to_string()),
                None,
                Some("#f97583".to_string()),
            ]
        );
    }

    /// Normalization reaches indexed_colors too: a `0x`-form indexed color
    /// must arrive at xterm as `#`-form, since xterm's `css.toColor` only
    /// understands `#`/`rgb()`/named CSS colors.
    #[test]
    fn indexed_colors_are_normalized_before_reaching_extended_ansi() {
        let toml_str = r##"
[colors.primary]
background = "#1e1e2e"
foreground = "#cdd6f4"

[colors.normal]
black = "#45475a"
red = "#f38ba8"
green = "#a6e3a1"
yellow = "#f9e2af"
blue = "#89b4fa"
magenta = "#f5c2e7"
cyan = "#94e2d5"
white = "#bac2de"

[colors.bright]
black = "#585b70"
red = "#f38ba8"
green = "#a6e3a1"
yellow = "#f9e2af"
blue = "#89b4fa"
magenta = "#f5c2e7"
cyan = "#94e2d5"
white = "#a6adc8"

[[colors.indexed_colors]]
index = 16
color = "0xd18616"

[[colors.indexed_colors]]
index = 17
color = "CellForeground"
"##;
        let tc = resolve_theme_colors_from_scheme(&load_theme_str("indexed", toml_str));
        assert_eq!(
            tc.extended_ansi,
            vec![Some("#d18616".to_string()), Some("#cdd6f4".to_string()),]
        );
    }

    // ---------------------------------------------------------------------
    // auto resolution through the command-facing entry point
    // ---------------------------------------------------------------------

    fn config_with_theme(theme: &str) -> UserConfig {
        UserConfig {
            colors: termlab_core::config::ColorsConfig {
                theme: theme.into(),
                ..Default::default()
            },
            ..UserConfig::default()
        }
    }

    #[test]
    fn auto_resolves_to_a_different_palette_per_appearance() {
        let cfg = config_with_theme("auto");
        let dark = resolve_theme_colors_for_appearance(&cfg, Some("dark"));
        let light = resolve_theme_colors_for_appearance(&cfg, Some("light"));
        assert_ne!(dark.background, light.background);
        assert_ne!(dark.foreground, light.foreground);
    }

    /// Back-compat: an invoke that omits `resolved_appearance` behaves as
    /// dark. This is the pin for every pre-existing caller.
    #[test]
    fn an_absent_appearance_argument_resolves_as_dark() {
        let cfg = config_with_theme("auto");
        let absent = resolve_theme_colors_for_appearance(&cfg, None);
        let dark = resolve_theme_colors_for_appearance(&cfg, Some("dark"));
        assert_eq!(absent.background, dark.background);
        assert_eq!(absent.foreground, dark.foreground);
        assert_eq!(absent.text_muted, dark.text_muted);
    }

    /// The decoupling guarantee, at the level the frontend actually consumes:
    /// a config naming a concrete theme produces a byte-identical payload
    /// under both appearances, so the re-theme fetch an appearance flip
    /// triggers is a no-op for that user.
    #[test]
    fn a_concrete_theme_name_yields_an_identical_payload_under_both_appearances() {
        let cfg = config_with_theme("TermLab Light");
        let dark = resolve_theme_colors_for_appearance(&cfg, Some("dark"));
        let light = resolve_theme_colors_for_appearance(&cfg, Some("light"));
        let dark_json = serde_json::to_value(&dark).expect("serializable");
        let light_json = serde_json::to_value(&light).expect("serializable");
        assert_eq!(dark_json, light_json);
        // And it really stayed the LIGHT palette under a dark appearance,
        // rather than being swapped for the dark built-in `auto` would pick.
        assert_eq!(dark.background, "#E3E8EF");
    }

    #[test]
    fn the_default_config_is_auto_and_therefore_appearance_tracking() {
        let cfg = UserConfig::default();
        assert_eq!(cfg.colors.theme, "auto");
        let dark = resolve_theme_colors_for_appearance(&cfg, Some("dark"));
        let light = resolve_theme_colors_for_appearance(&cfg, Some("light"));
        assert_ne!(dark.background, light.background);
    }
}
