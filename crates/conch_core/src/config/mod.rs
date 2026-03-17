//! Configuration and persistent state management.
//!
//! Split into two files on disk:
//! - `config.toml` — terminal + appearance prefs (Alacritty-compatible + [conch.*] extensions)
//! - `state.toml` — ephemeral UI state (not user-edited)

mod colors;
mod conch;
mod font;
mod persistent;
mod terminal;
mod window;

pub use colors::*;
pub use conch::*;
pub use font::*;
pub use persistent::*;
pub use terminal::*;
pub use window::*;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// UserConfig — ~/.config/conch/config.toml
// ---------------------------------------------------------------------------

/// User preferences (portable, version-controlled).
///
/// Terminal font can live at `[font]` (legacy) or `[terminal.font]` (preferred).
/// If `[terminal.font]` is at its default and `[font]` has been customized,
/// the legacy value is used.  Call [`UserConfig::resolved_terminal_font`] to
/// get the effective font config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    pub window: WindowConfig,
    /// Legacy top-level `[font]` section.  Prefer `[terminal.font]` instead.
    #[serde(default)]
    pub font: FontConfig,
    pub colors: ColorsConfig,
    pub terminal: TerminalConfig,
    pub conch: ConchConfig,
}

impl UserConfig {
    /// Return the effective terminal font config.
    ///
    /// Prefers `[terminal.font]` when set; falls back to legacy `[font]`.
    pub fn resolved_terminal_font(&self) -> &FontConfig {
        if self.terminal.font != FontConfig::default() {
            &self.terminal.font
        } else {
            &self.font
        }
    }
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            font: FontConfig::default(),
            colors: ColorsConfig::default(),
            terminal: TerminalConfig::default(),
            conch: ConchConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Returns the config directory.
///
/// - macOS / Linux: `~/.config/conch/`
/// - Windows: `%APPDATA%\conch\`
pub fn config_dir() -> PathBuf {
    #[cfg(not(target_os = "windows"))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".config")
            .join("conch")
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("conch")
    }
}

pub fn config_path() -> PathBuf { config_dir().join("config.toml") }
fn state_path() -> PathBuf { config_dir().join("state.toml") }

// ---------------------------------------------------------------------------
// Load / Save — UserConfig
// ---------------------------------------------------------------------------

pub fn load_user_config() -> Result<UserConfig> {
    let path = config_path();
    if !path.exists() {
        log::info!("No config.toml at {}, using defaults", path.display());
        return Ok(UserConfig::default());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let config: UserConfig = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(config)
}

pub fn save_user_config(config: &UserConfig) -> Result<()> {
    let dir = config_dir();
    if !dir.exists() { fs::create_dir_all(&dir)?; }
    let contents = toml::to_string_pretty(config).context("Failed to serialize config")?;
    fs::write(config_path(), contents)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Load / Save — PersistentState
// ---------------------------------------------------------------------------

pub fn load_persistent_state() -> Result<PersistentState> {
    let path = state_path();
    if !path.exists() {
        log::info!("No state.toml at {}, using defaults", path.display());
        return Ok(PersistentState::default());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let state: PersistentState = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(state)
}

pub fn save_persistent_state(state: &PersistentState) -> Result<()> {
    let dir = config_dir();
    if !dir.exists() { fs::create_dir_all(&dir)?; }
    let contents = toml::to_string_pretty(state).context("Failed to serialize state")?;
    fs::write(state_path(), contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_font_preferred_over_legacy() {
        let toml_str = r#"
            [font]
            size = 16.0

            [terminal.font]
            size = 18.0
        "#;
        let cfg: UserConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.resolved_terminal_font().size, 18.0);
    }

    #[test]
    fn legacy_font_used_when_terminal_font_default() {
        let toml_str = r#"
            [font]
            size = 16.0
        "#;
        let cfg: UserConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.resolved_terminal_font().size, 16.0);
    }

    #[test]
    fn default_font_when_neither_set() {
        let cfg = UserConfig::default();
        assert_eq!(cfg.resolved_terminal_font().size, FontConfig::default().size);
    }

    #[test]
    fn ui_font_from_full_config() {
        let toml_str = r#"
            [conch.ui.font]
            small = 10.0
            list = 12.0
            normal = 13.0
        "#;
        let cfg: UserConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.conch.ui.font.small, 10.0);
        assert_eq!(cfg.conch.ui.font.list, 12.0);
        assert_eq!(cfg.conch.ui.font.normal, 13.0);
    }
}
