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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    pub window: WindowConfig,
    pub font: FontConfig,
    pub colors: ColorsConfig,
    pub terminal: TerminalConfig,
    pub conch: ConchConfig,
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

/// Returns the config directory: `~/.config/conch/`.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("conch")
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
