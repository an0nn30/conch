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
