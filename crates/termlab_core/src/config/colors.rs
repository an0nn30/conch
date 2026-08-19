//! Color and appearance configuration.

use serde::{Deserialize, Serialize};

/// Application appearance mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AppearanceMode {
    Dark,
    Light,
    System,
}

impl Default for AppearanceMode {
    fn default() -> Self {
        Self::Dark
    }
}

impl<'de> Deserialize<'de> for AppearanceMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            "system" => Ok(Self::System),
            _ => Err(serde::de::Error::unknown_variant(
                &s,
                &["dark", "light", "system"],
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    pub theme: String,
    pub appearance_mode: AppearanceMode,
}

impl AppearanceMode {
    /// The best resolved appearance Rust alone can name for this mode.
    ///
    /// `System` resolves through `matchMedia` inside the webview, which Rust
    /// cannot see, so it degrades to `"dark"` — the same unresolvable-is-dark
    /// convention as `app/core/appearance.js` and
    /// `effective_theme::DEFAULT_RESOLVED_APPEARANCE`. Callers that CAN see
    /// the real resolved value (anything reached from the frontend) must pass
    /// that instead of using this.
    pub fn resolved_hint(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark | Self::System => "dark",
        }
    }
}

impl Default for ColorsConfig {
    /// `theme` defaults to the reserved `"auto"` name
    /// (`effective_theme::AUTO_THEME_NAME`), which tracks the app appearance
    /// across the two built-in TermLab palettes.
    ///
    /// This is a *serde* default only. `ColorsConfig` is `#[serde(default)]`,
    /// so a `config.toml` that names any theme keeps that theme untouched;
    /// only a config with no `colors.theme` key at all (including a brand-new
    /// install) picks up `auto`.
    fn default() -> Self {
        Self {
            theme: crate::effective_theme::AUTO_THEME_NAME.into(),
            appearance_mode: AppearanceMode::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper wrapper so we can deserialize a single field from TOML.
    #[derive(Deserialize)]
    struct ModeWrapper {
        mode: AppearanceMode,
    }

    fn parse_mode(toml_str: &str) -> Result<AppearanceMode, toml::de::Error> {
        let w: ModeWrapper = toml::from_str(toml_str)?;
        Ok(w.mode)
    }

    #[test]
    fn appearance_mode_default_is_dark() {
        assert_eq!(AppearanceMode::default(), AppearanceMode::Dark);
    }

    #[test]
    fn appearance_mode_deserialize_dark() {
        assert_eq!(
            parse_mode(r#"mode = "dark""#).unwrap(),
            AppearanceMode::Dark
        );
    }

    #[test]
    fn appearance_mode_deserialize_light() {
        assert_eq!(
            parse_mode(r#"mode = "light""#).unwrap(),
            AppearanceMode::Light
        );
    }

    #[test]
    fn appearance_mode_deserialize_system() {
        assert_eq!(
            parse_mode(r#"mode = "system""#).unwrap(),
            AppearanceMode::System
        );
    }

    #[test]
    fn appearance_mode_case_insensitive() {
        assert_eq!(
            parse_mode(r#"mode = "DARK""#).unwrap(),
            AppearanceMode::Dark
        );
        assert_eq!(
            parse_mode(r#"mode = "Light""#).unwrap(),
            AppearanceMode::Light
        );
        assert_eq!(
            parse_mode(r#"mode = "SYSTEM""#).unwrap(),
            AppearanceMode::System
        );
    }

    #[test]
    fn appearance_mode_invalid_value_errors() {
        assert!(parse_mode(r#"mode = "purple""#).is_err());
    }

    #[test]
    fn colors_config_default_theme_is_auto() {
        let c = ColorsConfig::default();
        assert_eq!(c.theme, "auto");
    }

    /// The default flip is serde-only. A config file that omits
    /// `colors.theme` gets `auto`; a config file that names a theme keeps it
    /// byte for byte, with no migration.
    #[test]
    fn a_config_without_the_theme_key_gets_auto() {
        let parsed: ColorsConfig = toml::from_str(r#"appearance_mode = "light""#).unwrap();
        assert_eq!(parsed.theme, "auto");
        assert_eq!(parsed.appearance_mode, AppearanceMode::Light);
    }

    #[test]
    fn a_config_naming_a_theme_is_left_untouched_by_the_default_flip() {
        for name in ["gruvbox_dark", "TermLab Dark", "TermLab Light"] {
            let parsed: ColorsConfig =
                toml::from_str(&format!("theme = {}", toml::Value::from(name))).unwrap();
            assert_eq!(parsed.theme, name, "{name} must survive the default flip");
        }
    }

    /// Saving ANY setting materializes `theme = "auto"` into a config file
    /// that previously had no `colors.theme` key at all. `save_settings`
    /// deserializes the whole `UserConfig` and re-serializes it, so every
    /// `#[serde(default)]` field becomes explicit on the next write — a
    /// pre-existing pattern that predates this key, not something `auto`
    /// introduced.
    ///
    /// This is DELIBERATE and it is downgrade-safe. A build that predates
    /// the reserved name treats `"auto"` as an ordinary theme name, finds no
    /// `auto.toml`, and falls back to its own built-in default palette —
    /// which is exactly what that same build showed for the keyless config
    /// before the save, since the old serde default resolved to that same
    /// hardcoded palette too. So the materialized key changes nothing for an
    /// older binary reading the same file.
    #[test]
    fn a_keyless_config_materializes_theme_auto_on_the_next_save() {
        let parsed: ColorsConfig = toml::from_str(r#"appearance_mode = "dark""#).unwrap();
        let written = toml::to_string(&parsed).unwrap();
        assert!(
            written.contains(r#"theme = "auto""#),
            "the next save writes the key explicitly; got:\n{written}"
        );

        // And it round-trips to the same value rather than drifting.
        let reparsed: ColorsConfig = toml::from_str(&written).unwrap();
        assert_eq!(reparsed.theme, "auto");
        assert_eq!(reparsed.appearance_mode, parsed.appearance_mode);
    }

    #[test]
    fn an_empty_colors_table_gets_auto_and_dark() {
        let parsed: ColorsConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.theme, "auto");
        assert_eq!(parsed.appearance_mode, AppearanceMode::Dark);
    }

    #[test]
    fn resolved_hint_is_light_only_for_light() {
        assert_eq!(AppearanceMode::Light.resolved_hint(), "light");
        assert_eq!(AppearanceMode::Dark.resolved_hint(), "dark");
        assert_eq!(
            AppearanceMode::System.resolved_hint(),
            "dark",
            "Rust cannot see matchMedia, so System degrades to dark"
        );
    }

    #[test]
    fn colors_config_roundtrip() {
        let cfg = ColorsConfig::default();
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: ColorsConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.appearance_mode, cfg.appearance_mode);
        assert_eq!(parsed.theme, cfg.theme);
    }
}
