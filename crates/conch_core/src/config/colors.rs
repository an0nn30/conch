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

impl Default for ColorsConfig {
    fn default() -> Self {
        Self {
            theme: "dracula".into(),
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
        assert_eq!(parse_mode(r#"mode = "dark""#).unwrap(), AppearanceMode::Dark);
    }

    #[test]
    fn appearance_mode_deserialize_light() {
        assert_eq!(parse_mode(r#"mode = "light""#).unwrap(), AppearanceMode::Light);
    }

    #[test]
    fn appearance_mode_deserialize_system() {
        assert_eq!(parse_mode(r#"mode = "system""#).unwrap(), AppearanceMode::System);
    }

    #[test]
    fn appearance_mode_case_insensitive() {
        assert_eq!(parse_mode(r#"mode = "DARK""#).unwrap(), AppearanceMode::Dark);
        assert_eq!(parse_mode(r#"mode = "Light""#).unwrap(), AppearanceMode::Light);
        assert_eq!(parse_mode(r#"mode = "SYSTEM""#).unwrap(), AppearanceMode::System);
    }

    #[test]
    fn appearance_mode_invalid_value_errors() {
        assert!(parse_mode(r#"mode = "purple""#).is_err());
    }

    #[test]
    fn colors_config_default_theme() {
        let c = ColorsConfig::default();
        assert_eq!(c.theme, "dracula");
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
