//! Alacritty-compatible color theme loading.
//!
//! Deserializes unmodified Alacritty `.toml` theme files (e.g. dracula.toml,
//! catppuccin_mocha.toml) and provides a built-in Dracula fallback.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level wrapper matching Alacritty theme file structure.
#[derive(Debug, Clone, Deserialize)]
pub struct AlacrittyThemeFile {
    pub colors: ColorScheme,
}

/// Full color scheme with primary, normal, bright, and optional dim/cursor/selection.
#[derive(Debug, Clone, Deserialize)]
pub struct ColorScheme {
    pub primary: PrimaryColors,
    pub normal: AnsiColors,
    pub bright: AnsiColors,
    #[serde(default)]
    pub dim: Option<AnsiColors>,
    #[serde(default)]
    pub cursor: Option<CursorColors>,
    #[serde(default)]
    pub selection: Option<SelectionColors>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrimaryColors {
    pub background: String,
    pub foreground: String,
    #[serde(default)]
    pub dim_foreground: Option<String>,
    #[serde(default)]
    pub bright_foreground: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnsiColors {
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CursorColors {
    pub text: String,
    pub cursor: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SelectionColors {
    pub text: String,
    pub background: String,
}

impl AnsiColors {
    /// Return the 8 colors as an array in ANSI order.
    pub fn as_array(&self) -> [&str; 8] {
        [
            &self.black,
            &self.red,
            &self.green,
            &self.yellow,
            &self.blue,
            &self.magenta,
            &self.cyan,
            &self.white,
        ]
    }
}

impl Default for ColorScheme {
    /// Built-in Dracula theme matching the real `dracula.toml`.
    fn default() -> Self {
        Self {
            primary: PrimaryColors {
                background: "#282a36".into(),
                foreground: "#f8f8f2".into(),
                dim_foreground: Some("#6272a4".into()),
                bright_foreground: Some("#ffffff".into()),
            },
            normal: AnsiColors {
                black: "#21222c".into(),
                red: "#ff5555".into(),
                green: "#50fa7b".into(),
                yellow: "#f1fa8c".into(),
                blue: "#bd93f9".into(),
                magenta: "#ff79c6".into(),
                cyan: "#8be9fd".into(),
                white: "#f8f8f2".into(),
            },
            bright: AnsiColors {
                black: "#6272a4".into(),
                red: "#ff6e6e".into(),
                green: "#69ff94".into(),
                yellow: "#ffffa5".into(),
                blue: "#d6acff".into(),
                magenta: "#ff92df".into(),
                cyan: "#a4ffff".into(),
                white: "#ffffff".into(),
            },
            dim: None,
            cursor: Some(CursorColors {
                text: "#282a36".into(),
                cursor: "#f8f8f2".into(),
            }),
            selection: Some(SelectionColors {
                text: "#f8f8f2".into(),
                background: "#44475a".into(),
            }),
        }
    }
}

/// Return the themes directory: `~/.config/conch/themes/`.
pub fn themes_dir() -> PathBuf {
    crate::config::config_dir().join("themes")
}

/// Load a color scheme from an Alacritty-format TOML file.
pub fn load_theme(path: &Path) -> Result<ColorScheme> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read theme from {}", path.display()))?;
    let theme_file: AlacrittyThemeFile = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse theme from {}", path.display()))?;
    Ok(theme_file.colors)
}

/// Scan the themes directory and return a map of `name -> path`.
pub fn list_themes() -> HashMap<String, PathBuf> {
    let dir = themes_dir();
    let mut themes = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                if let Some(stem) = path.file_stem() {
                    themes.insert(stem.to_string_lossy().into_owned(), path);
                }
            }
        }
    }
    themes
}

/// Resolve a theme by name or path: load from disk or fall back to built-in Dracula.
///
/// If `value` is a file path (contains `/`, `\`, or ends with `.toml`), it is
/// loaded directly. A leading `~` is expanded to the home directory.
/// Otherwise `value` is treated as a theme name and looked up in the themes
/// directory (`~/.config/conch/themes/{name}.toml`).
pub fn resolve_theme(value: &str) -> ColorScheme {
    let is_path = value.contains('/') || value.contains('\\') || value.ends_with(".toml");

    if is_path {
        let expanded = if value.starts_with("~/") {
            dirs::home_dir()
                .map(|h| h.join(&value[2..]))
                .unwrap_or_else(|| PathBuf::from(value))
        } else {
            PathBuf::from(value)
        };
        match load_theme(&expanded) {
            Ok(scheme) => {
                log::info!("Loaded theme from {}", expanded.display());
                return scheme;
            }
            Err(e) => {
                log::warn!(
                    "Failed to load theme from '{}': {e}, using built-in Dracula",
                    expanded.display()
                );
            }
        }
    } else {
        let themes = list_themes();
        if let Some(path) = themes.get(value) {
            match load_theme(path) {
                Ok(scheme) => {
                    log::info!("Loaded theme '{}' from {}", value, path.display());
                    return scheme;
                }
                Err(e) => {
                    log::warn!(
                        "Failed to load theme '{}': {e}, using built-in Dracula",
                        value
                    );
                }
            }
        } else if !value.eq_ignore_ascii_case("dracula") {
            log::info!(
                "Theme '{}' not found in themes dir, using built-in Dracula",
                value
            );
        }
    }
    ColorScheme::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_color_scheme_primary_colors() {
        let cs = ColorScheme::default();
        assert_eq!(cs.primary.background, "#282a36", "Dracula background");
        assert_eq!(cs.primary.foreground, "#f8f8f2", "Dracula foreground");
        assert_eq!(
            cs.primary.dim_foreground.as_deref(),
            Some("#6272a4"),
            "Dracula dim foreground"
        );
        assert_eq!(
            cs.primary.bright_foreground.as_deref(),
            Some("#ffffff"),
            "Dracula bright foreground"
        );
    }

    #[test]
    fn default_color_scheme_optional_fields() {
        let cs = ColorScheme::default();
        assert!(cs.dim.is_none(), "dim should be None by default");
        assert!(cs.cursor.is_some(), "cursor should be Some by default");
        assert!(
            cs.selection.is_some(),
            "selection should be Some by default"
        );

        let cursor = cs.cursor.unwrap();
        assert_eq!(cursor.text, "#282a36");
        assert_eq!(cursor.cursor, "#f8f8f2");

        let selection = cs.selection.unwrap();
        assert_eq!(selection.text, "#f8f8f2");
        assert_eq!(selection.background, "#44475a");
    }

    #[test]
    fn ansi_colors_as_array() {
        let cs = ColorScheme::default();
        let normal = cs.normal.as_array();
        assert_eq!(normal.len(), 8);
        assert_eq!(normal[0], "#21222c", "black");
        assert_eq!(normal[1], "#ff5555", "red");
        assert_eq!(normal[2], "#50fa7b", "green");
        assert_eq!(normal[3], "#f1fa8c", "yellow");
        assert_eq!(normal[4], "#bd93f9", "blue");
        assert_eq!(normal[5], "#ff79c6", "magenta");
        assert_eq!(normal[6], "#8be9fd", "cyan");
        assert_eq!(normal[7], "#f8f8f2", "white");
    }

    #[test]
    fn deserialize_complete_alacritty_theme() {
        let toml_str = r##"
[colors.primary]
background = "#1e1e2e"
foreground = "#cdd6f4"

[colors.normal]
black   = "#45475a"
red     = "#f38ba8"
green   = "#a6e3a1"
yellow  = "#f9e2af"
blue    = "#89b4fa"
magenta = "#f5c2e7"
cyan    = "#94e2d5"
white   = "#bac2de"

[colors.bright]
black   = "#585b70"
red     = "#f38ba8"
green   = "#a6e3a1"
yellow  = "#f9e2af"
blue    = "#89b4fa"
magenta = "#f5c2e7"
cyan    = "#94e2d5"
white   = "#a6adc8"

[colors.cursor]
text   = "#1e1e2e"
cursor = "#f5e0dc"

[colors.selection]
text       = "#1e1e2e"
background = "#f5e0dc"
"##;
        let theme: AlacrittyThemeFile = toml::from_str(toml_str).expect("valid TOML");
        assert_eq!(theme.colors.primary.background, "#1e1e2e");
        assert_eq!(theme.colors.primary.foreground, "#cdd6f4");
        assert_eq!(theme.colors.normal.black, "#45475a");
        assert_eq!(theme.colors.bright.white, "#a6adc8");
        assert!(theme.colors.cursor.is_some());
        assert!(theme.colors.selection.is_some());
    }

    #[test]
    fn deserialize_missing_optional_fields() {
        let toml_str = r##"
[colors.primary]
background = "#000000"
foreground = "#ffffff"

[colors.normal]
black   = "#000000"
red     = "#ff0000"
green   = "#00ff00"
yellow  = "#ffff00"
blue    = "#0000ff"
magenta = "#ff00ff"
cyan    = "#00ffff"
white   = "#ffffff"

[colors.bright]
black   = "#808080"
red     = "#ff0000"
green   = "#00ff00"
yellow  = "#ffff00"
blue    = "#0000ff"
magenta = "#ff00ff"
cyan    = "#00ffff"
white   = "#ffffff"
"##;
        let theme: AlacrittyThemeFile = toml::from_str(toml_str).expect("valid TOML");
        let cs = theme.colors;
        assert!(cs.dim.is_none(), "dim should be None when absent from TOML");
        assert!(
            cs.cursor.is_none(),
            "cursor should be None when absent from TOML"
        );
        assert!(
            cs.selection.is_none(),
            "selection should be None when absent from TOML"
        );
        assert!(
            cs.primary.dim_foreground.is_none(),
            "dim_foreground should be None when absent"
        );
        assert!(
            cs.primary.bright_foreground.is_none(),
            "bright_foreground should be None when absent"
        );
    }

    #[test]
    fn deserialize_with_dim_colors() {
        let toml_str = r##"
[colors.primary]
background = "#000000"
foreground = "#ffffff"
dim_foreground = "#aaaaaa"

[colors.normal]
black = "#000"
red = "#f00"
green = "#0f0"
yellow = "#ff0"
blue = "#00f"
magenta = "#f0f"
cyan = "#0ff"
white = "#fff"

[colors.bright]
black = "#888"
red = "#f00"
green = "#0f0"
yellow = "#ff0"
blue = "#00f"
magenta = "#f0f"
cyan = "#0ff"
white = "#fff"

[colors.dim]
black = "#111"
red = "#a00"
green = "#0a0"
yellow = "#aa0"
blue = "#00a"
magenta = "#a0a"
cyan = "#0aa"
white = "#aaa"
"##;
        let theme: AlacrittyThemeFile = toml::from_str(toml_str).expect("valid TOML");
        let cs = theme.colors;
        assert!(cs.dim.is_some(), "dim should be present");
        let dim = cs.dim.unwrap();
        assert_eq!(dim.black, "#111");
        assert_eq!(dim.white, "#aaa");
        assert_eq!(cs.primary.dim_foreground.as_deref(), Some("#aaaaaa"));
    }
}
