//! Font configuration: family, size, and cell offset.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontFamily {
    pub family: String,
}

impl Default for FontFamily {
    fn default() -> Self {
        Self {
            family: "JetBrains Mono".into(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_family_default() {
        let ff = FontFamily::default();
        assert_eq!(ff.family, "JetBrains Mono");
    }

    #[test]
    fn font_offset_default() {
        let fo = FontOffset::default();
        assert_eq!(fo.x, 0.0);
        assert_eq!(fo.y, 0.0);
    }

    #[test]
    fn font_config_default() {
        let fc = FontConfig::default();
        assert_eq!(fc.normal.family, "JetBrains Mono");
        assert_eq!(fc.size, 14.0);
        assert_eq!(fc.offset.x, 0.0);
        assert_eq!(fc.offset.y, 0.0);
    }

    #[test]
    fn font_config_serde_round_trip() {
        let original = FontConfig {
            normal: FontFamily {
                family: "Fira Code".into(),
            },
            size: 16.5,
            offset: FontOffset { x: 1.0, y: -0.5 },
        };
        let toml_str = toml::to_string(&original).expect("serialize");
        let restored: FontConfig = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn font_config_deserialize_partial_toml() {
        // Only set size; normal and offset should use defaults
        let toml_str = "size = 18.0\n";
        let fc: FontConfig = toml::from_str(toml_str).expect("deserialize partial");
        assert_eq!(fc.size, 18.0);
        assert_eq!(fc.normal.family, "JetBrains Mono", "default family");
        assert_eq!(fc.offset.x, 0.0, "default offset x");
        assert_eq!(fc.offset.y, 0.0, "default offset y");
    }

    #[test]
    fn font_config_deserialize_empty_toml() {
        let fc: FontConfig = toml::from_str("").expect("deserialize empty");
        assert_eq!(fc, FontConfig::default());
    }
}
