//! Resolving the CONFIGURED terminal theme name into the EFFECTIVE one.
//!
//! `colors.theme` is a free-form string with one reserved value: `"auto"`,
//! which means "follow the app's resolved appearance" and maps onto the two
//! built-in palettes shipped in the frontend `themes/` directory.
//!
//! Appearance is deliberately an *argument*, not something looked up here.
//! `AppearanceMode::System` resolves through `matchMedia('(prefers-color-scheme:
//! dark)')` inside the webview; Rust cannot see that, so the frontend passes
//! the value it already resolved (`termlabAppearance.current()`) down with the
//! request. Rust never guesses.
//!
//! Everything that is NOT `"auto"` is handed to
//! [`crate::color_scheme::resolve_theme`] completely unchanged — same lookup
//! order, same collision rule, same silent fallback to the built-in default
//! palette. A config naming a concrete theme therefore resolves to the
//! identical palette under both appearances, which is the spec's decoupling
//! guarantee.

use crate::color_scheme::{ColorScheme, resolve_theme};
use crate::config::ColorsConfig;

/// The reserved `colors.theme` value meaning "track the app appearance".
pub const AUTO_THEME_NAME: &str = "auto";
/// The built-in palette `auto` selects under a dark appearance.
pub const TERMLAB_DARK_THEME: &str = "TermLab Dark";
/// The built-in palette `auto` selects under a light appearance.
pub const TERMLAB_LIGHT_THEME: &str = "TermLab Light";

/// The appearance string used when a caller supplies none.
///
/// Same convention as `app/core/appearance.js` and
/// `config_service::resolveNativeWindowTheme`: `"light"` is the only
/// affirmative light answer, and anything unresolvable is dark.
pub const DEFAULT_RESOLVED_APPEARANCE: &str = "dark";

/// Map a resolved-appearance string onto the built-in palette `auto` uses.
fn builtin_for_appearance(resolved_appearance: &str) -> &'static str {
    if resolved_appearance.trim().eq_ignore_ascii_case("light") {
        TERMLAB_LIGHT_THEME
    } else {
        TERMLAB_DARK_THEME
    }
}

/// Translate a configured theme name into the name that should actually be
/// looked up.
///
/// `"auto"` (matched case-insensitively — it is a reserved name, not a file
/// stem) becomes `"TermLab Dark"` or `"TermLab Light"`. Every other value is
/// returned untouched, including names that do not exist: resolution, not
/// this function, owns the fallback behavior.
pub fn effective_theme_name<'a>(configured: &'a str, resolved_appearance: &str) -> &'a str {
    if configured.trim().eq_ignore_ascii_case(AUTO_THEME_NAME) {
        builtin_for_appearance(resolved_appearance)
    } else {
        configured
    }
}

/// Resolve `colors.theme` into a loaded [`ColorScheme`], honoring `"auto"`.
///
/// Equivalent to `resolve_theme(effective_theme_name(...))` — the whole point
/// is that the `auto` translation happens before, and never inside, the
/// existing lookup.
pub fn resolve_effective_theme(colors: &ColorsConfig, resolved_appearance: &str) -> ColorScheme {
    resolve_theme(effective_theme_name(&colors.theme, resolved_appearance))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(theme: &str) -> ColorsConfig {
        ColorsConfig {
            theme: theme.into(),
            ..ColorsConfig::default()
        }
    }

    #[test]
    fn auto_under_dark_selects_the_dark_builtin() {
        assert_eq!(effective_theme_name("auto", "dark"), TERMLAB_DARK_THEME);
    }

    #[test]
    fn auto_under_light_selects_the_light_builtin() {
        assert_eq!(effective_theme_name("auto", "light"), TERMLAB_LIGHT_THEME);
    }

    #[test]
    fn auto_with_an_unresolvable_appearance_falls_back_to_dark() {
        for appearance in ["", "system", "sepia", "DARK"] {
            assert_eq!(
                effective_theme_name("auto", appearance),
                TERMLAB_DARK_THEME,
                "{appearance:?} is not an affirmative light answer"
            );
        }
    }

    #[test]
    fn the_appearance_string_is_matched_case_insensitively() {
        assert_eq!(effective_theme_name("auto", "Light"), TERMLAB_LIGHT_THEME);
        assert_eq!(effective_theme_name("auto", "LIGHT"), TERMLAB_LIGHT_THEME);
        assert_eq!(effective_theme_name("auto", " light "), TERMLAB_LIGHT_THEME);
    }

    #[test]
    fn auto_is_a_reserved_name_matched_case_insensitively() {
        assert_eq!(effective_theme_name("Auto", "light"), TERMLAB_LIGHT_THEME);
        assert_eq!(effective_theme_name("AUTO", "dark"), TERMLAB_DARK_THEME);
    }

    #[test]
    fn a_concrete_name_is_returned_untouched_under_both_appearances() {
        for name in [
            "gruvbox_dark",
            "TermLab Dark",
            "TermLab Light",
            "~/mine.toml",
        ] {
            assert_eq!(effective_theme_name(name, "dark"), name);
            assert_eq!(effective_theme_name(name, "light"), name);
        }
    }

    /// The decoupling guarantee, at the level that actually loads files: a
    /// config naming a concrete theme must produce the SAME palette under
    /// both appearances. `"TermLab Light"` is used because it is a bundled
    /// built-in (so it resolves without touching the user's config
    /// directory) AND it is the palette `auto` would pick under only ONE of
    /// the two appearances — so a regression that leaked appearance into a
    /// concrete name's resolution shows up as a changed background here,
    /// rather than coincidentally matching.
    #[test]
    fn a_concrete_name_resolves_to_an_identical_palette_under_both_appearances() {
        let cfg = colors(TERMLAB_LIGHT_THEME);
        let dark = resolve_effective_theme(&cfg, "dark");
        let light = resolve_effective_theme(&cfg, "light");
        assert_eq!(dark.primary.background, light.primary.background);
        assert_eq!(dark.primary.foreground, light.primary.foreground);
        assert_eq!(dark.normal.as_array(), light.normal.as_array());
        assert_eq!(dark.bright.as_array(), light.bright.as_array());
        // And specifically: it stayed the LIGHT palette under a dark
        // appearance, rather than being swapped for the dark built-in.
        assert_eq!(dark.primary.background, "#E3E8EF");
    }

    /// `auto` must actually change the loaded palette, not merely the name.
    /// Both built-ins ship in the frontend `themes/` directory, so this reads
    /// real files through the real lookup.
    #[test]
    fn auto_resolves_to_different_palettes_under_the_two_appearances() {
        let cfg = colors(AUTO_THEME_NAME);
        let dark = resolve_effective_theme(&cfg, "dark");
        let light = resolve_effective_theme(&cfg, "light");
        assert_ne!(
            dark.primary.background, light.primary.background,
            "auto must track appearance"
        );
        // Sanity: the light palette really is the light one.
        let dark_bg = u32::from_str_radix(dark.primary.background.trim_start_matches('#'), 16)
            .expect("dark background is canonical hex");
        let light_bg = u32::from_str_radix(light.primary.background.trim_start_matches('#'), 16)
            .expect("light background is canonical hex");
        assert!(
            light_bg > dark_bg,
            "TermLab Light's background ({}) should be brighter than TermLab Dark's ({})",
            light.primary.background,
            dark.primary.background
        );
    }

    #[test]
    fn the_default_config_is_auto_and_therefore_appearance_tracking() {
        let cfg = ColorsConfig::default();
        assert_eq!(cfg.theme, AUTO_THEME_NAME);
        assert_eq!(
            effective_theme_name(&cfg.theme, "light"),
            TERMLAB_LIGHT_THEME
        );
        assert_eq!(effective_theme_name(&cfg.theme, "dark"), TERMLAB_DARK_THEME);
    }
}
