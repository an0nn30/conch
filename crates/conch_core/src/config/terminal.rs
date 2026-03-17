//! Terminal configuration: shell, cursor, scroll, and environment.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::FontConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub shell: TerminalShell,
    pub env: HashMap<String, String>,
    pub cursor: CursorConfig,
    pub scroll_sensitivity: f32,
    pub font: FontConfig,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: TerminalShell::default(),
            env: HashMap::new(),
            cursor: CursorConfig::default(),
            scroll_sensitivity: 0.15,
            font: FontConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalShell {
    pub program: String,
    pub args: Vec<String>,
}

impl Default for TerminalShell {
    fn default() -> Self {
        Self {
            program: String::new(),
            args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    pub style: CursorStyleConfig,
    pub vi_mode_style: Option<CursorStyleConfig>,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyleConfig::default(),
            vi_mode_style: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorStyleConfig {
    pub shape: String,
    #[serde(deserialize_with = "deserialize_blinking")]
    pub blinking: bool,
}

impl Default for CursorStyleConfig {
    fn default() -> Self {
        Self {
            shape: "Block".into(),
            blinking: true,
        }
    }
}

fn deserialize_blinking<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    use serde::de;

    struct BlinkingVisitor;

    impl<'de> de::Visitor<'de> for BlinkingVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a boolean or one of \"Never\", \"Off\", \"On\", \"Always\"")
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<bool, E> {
            match v.to_lowercase().as_str() {
                "always" | "on" => Ok(true),
                "never" | "off" => Ok(false),
                _ => Err(de::Error::unknown_variant(v, &["Never", "Off", "On", "Always"])),
            }
        }
    }

    deserializer.deserialize_any(BlinkingVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_style_default() {
        let s = CursorStyleConfig::default();
        assert_eq!(s.shape, "Block");
        assert!(s.blinking);
    }

    #[test]
    fn terminal_config_default_scroll() {
        assert_eq!(TerminalConfig::default().scroll_sensitivity, 0.15);
    }

    #[test]
    fn blinking_deserialize_true() {
        let cfg: CursorStyleConfig = toml::from_str(r#"shape = "Block"
blinking = true"#).unwrap();
        assert!(cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_false() {
        let cfg: CursorStyleConfig = toml::from_str(r#"shape = "Block"
blinking = false"#).unwrap();
        assert!(!cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_always_string() {
        let cfg: CursorStyleConfig = toml::from_str(r#"shape = "Block"
blinking = "Always""#).unwrap();
        assert!(cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_on_string() {
        let cfg: CursorStyleConfig = toml::from_str(r#"shape = "Block"
blinking = "On""#).unwrap();
        assert!(cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_never_string() {
        let cfg: CursorStyleConfig = toml::from_str(r#"shape = "Block"
blinking = "Never""#).unwrap();
        assert!(!cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_off_string() {
        let cfg: CursorStyleConfig = toml::from_str(r#"shape = "Block"
blinking = "off""#).unwrap();
        assert!(!cfg.blinking);
    }

    #[test]
    fn blinking_deserialize_invalid_string_errors() {
        let result: Result<CursorStyleConfig, _> = toml::from_str(r#"shape = "Block"
blinking = "maybe""#);
        assert!(result.is_err());
    }

    #[test]
    fn shell_default_empty() {
        let s = TerminalShell::default();
        assert!(s.program.is_empty());
        assert!(s.args.is_empty());
    }

    #[test]
    fn terminal_config_roundtrip() {
        let cfg = TerminalConfig::default();
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: TerminalConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.scroll_sensitivity, cfg.scroll_sensitivity);
        assert_eq!(parsed.shell, cfg.shell);
    }
}
