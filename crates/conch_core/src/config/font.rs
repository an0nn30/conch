//! Font configuration: family, size, and cell offset.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontFamily {
    pub family: String,
}

impl Default for FontFamily {
    fn default() -> Self {
        Self { family: "JetBrains Mono".into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontOffset {
    pub x: f32,
    pub y: f32,
}

impl Default for FontOffset {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub normal: FontFamily,
    pub size: f32,
    pub offset: FontOffset,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            normal: FontFamily::default(),
            size: 14.0,
            offset: FontOffset::default(),
        }
    }
}
