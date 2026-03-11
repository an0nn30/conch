//! Terminal configuration: shell, cursor, scroll, and environment.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub shell: TerminalShell,
    pub env: HashMap<String, String>,
    pub cursor: CursorConfig,
    pub scroll_sensitivity: f32,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: TerminalShell::default(),
            env: HashMap::new(),
            cursor: CursorConfig::default(),
            scroll_sensitivity: 0.15,
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
