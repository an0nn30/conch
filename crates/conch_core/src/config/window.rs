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
        Self { columns: 150, lines: 50 }
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
