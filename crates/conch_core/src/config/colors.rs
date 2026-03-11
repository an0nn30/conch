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
