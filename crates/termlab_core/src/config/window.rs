//! Window configuration: decorations and initial dimensions.

use serde::{Deserialize, Serialize};

/// Window decoration style (mirrors Alacritty `window.decorations`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub enum WindowDecorations {
    #[default]
    Full,
    Transparent,
    Buttonless,
    None,
}

impl<'de> Deserialize<'de> for WindowDecorations {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "transparent" => Ok(Self::Transparent),
            "buttonless" => Ok(Self::Buttonless),
            "none" => Ok(Self::None),
            _ => Err(serde::de::Error::unknown_variant(
                &s,
                &["Full", "Transparent", "Buttonless", "None"],
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowDimensions {
    pub columns: u16,
    pub lines: u16,
}

impl Default for WindowDimensions {
    fn default() -> Self {
        Self {
            columns: 150,
            lines: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub dimensions: WindowDimensions,
    pub decorations: WindowDecorations,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            dimensions: WindowDimensions::default(),
            decorations: WindowDecorations::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct DecWrapper {
        decorations: WindowDecorations,
    }

    fn parse_dec(toml_str: &str) -> Result<WindowDecorations, toml::de::Error> {
        let w: DecWrapper = toml::from_str(toml_str)?;
        Ok(w.decorations)
    }

    #[test]
    fn decorations_default_is_full() {
        assert_eq!(WindowDecorations::default(), WindowDecorations::Full);
    }

    #[test]
    fn decorations_deserialize_full() {
        assert_eq!(
            parse_dec(r#"decorations = "Full""#).unwrap(),
            WindowDecorations::Full
        );
    }

    #[test]
    fn decorations_deserialize_case_insensitive() {
        assert_eq!(
            parse_dec(r#"decorations = "transparent""#).unwrap(),
            WindowDecorations::Transparent
        );
        assert_eq!(
            parse_dec(r#"decorations = "BUTTONLESS""#).unwrap(),
            WindowDecorations::Buttonless
        );
    }

    #[test]
    fn decorations_deserialize_none() {
        assert_eq!(
            parse_dec(r#"decorations = "none""#).unwrap(),
            WindowDecorations::None
        );
    }

    #[test]
    fn decorations_deserialize_buttonless() {
        assert_eq!(
            parse_dec(r#"decorations = "buttonless""#).unwrap(),
            WindowDecorations::Buttonless
        );
    }

    #[test]
    fn decorations_invalid_value_errors() {
        assert!(parse_dec(r#"decorations = "fancy""#).is_err());
    }

    #[test]
    fn dimensions_default() {
        let d = WindowDimensions::default();
        assert_eq!(d.columns, 150);
        assert_eq!(d.lines, 50);
    }

    #[test]
    fn window_config_roundtrip() {
        let cfg = WindowConfig::default();
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: WindowConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.dimensions, cfg.dimensions);
    }
}
