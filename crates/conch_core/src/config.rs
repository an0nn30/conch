//! Configuration and persistent state management.
//!
//! Split into two files:
//! - `config.toml` — user preferences (font, theme, keyboard shortcuts)
//! - `state.toml` — UI state (panel layout, session history, folders, tunnels)
//!
//! Legacy single-file `config.toml` with `[general]` section is automatically migrated.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::models::{SavedTunnel, ServerFolder};

// ---------------------------------------------------------------------------
// UserConfig — ~/.config/conch/config.toml
// ---------------------------------------------------------------------------

/// User preferences (portable, version-controlled).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub font: FontConfig,
    #[serde(default)]
    pub colors: ColorsConfig,
    #[serde(default)]
    pub keyboard: KeyboardConfig,
    #[serde(default)]
    pub session: SessionSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSettings {
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub startup_command: String,
    #[serde(default)]
    pub use_tmux: bool,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            shell: String::new(),
            startup_command: String::new(),
            use_tmux: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontConfig {
    #[serde(default)]
    pub normal: FontFamily,
    #[serde(default = "default_font_size")]
    pub size: f32,
    #[serde(default)]
    pub ui_family: String,
    #[serde(default = "default_ui_size")]
    pub ui_size: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontFamily {
    #[serde(default = "default_font_name")]
    pub family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorsConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardConfig {
    #[serde(default = "default_new_tab")]
    pub new_tab: String,
    #[serde(default = "default_close_tab")]
    pub close_tab: String,
    #[serde(default = "default_new_connection")]
    pub new_connection: String,
    #[serde(default = "default_quit")]
    pub quit: String,
    #[serde(default = "default_toggle_left_sidebar")]
    pub toggle_left_sidebar: String,
    #[serde(default = "default_toggle_right_sidebar")]
    pub toggle_right_sidebar: String,
    #[serde(default = "default_focus_quick_connect")]
    pub focus_quick_connect: String,
}

fn default_theme() -> String { "dracula".into() }
fn default_font_size() -> f32 { 14.0 }
fn default_font_name() -> String { "JetBrains Mono".into() }
fn default_ui_size() -> f32 { 13.0 }
fn default_new_tab() -> String { "cmd+t".into() }
fn default_close_tab() -> String { "cmd+w".into() }
fn default_new_connection() -> String { "cmd+n".into() }
fn default_quit() -> String { "cmd+q".into() }
fn default_toggle_left_sidebar() -> String { "cmd+shift+b".into() }
fn default_toggle_right_sidebar() -> String { "cmd+shift+e".into() }
fn default_focus_quick_connect() -> String { "cmd+/".into() }

impl Default for FontFamily {
    fn default() -> Self { Self { family: default_font_name() } }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            normal: FontFamily::default(),
            size: default_font_size(),
            ui_family: String::new(),
            ui_size: default_ui_size(),
        }
    }
}

impl Default for ColorsConfig {
    fn default() -> Self { Self { theme: default_theme() } }
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            new_tab: default_new_tab(),
            close_tab: default_close_tab(),
            new_connection: default_new_connection(),
            quit: default_quit(),
            toggle_left_sidebar: default_toggle_left_sidebar(),
            toggle_right_sidebar: default_toggle_right_sidebar(),
            focus_quick_connect: default_focus_quick_connect(),
        }
    }
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            font: FontConfig::default(),
            colors: ColorsConfig::default(),
            keyboard: KeyboardConfig::default(),
            session: SessionSettings::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// PersistentState — ~/.config/conch/state.toml
// ---------------------------------------------------------------------------

/// Machine-local UI state (not version-controlled).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentState {
    #[serde(default)]
    pub layout: LayoutConfig,
    #[serde(default)]
    pub sessions: SessionConfig,
    #[serde(default)]
    pub folders: Vec<ServerFolder>,
    #[serde(default)]
    pub tunnels: Vec<SavedTunnel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutConfig {
    #[serde(default = "default_panel_width")]
    pub left_panel_width: f32,
    #[serde(default)]
    pub left_panel_collapsed: bool,
    #[serde(default)]
    pub right_panel_collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub last_session_keys: Vec<String>,
}

fn default_panel_width() -> f32 { 260.0 }

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            left_panel_width: default_panel_width(),
            left_panel_collapsed: false,
            right_panel_collapsed: false,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self { Self { last_session_keys: Vec::new() } }
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            layout: LayoutConfig::default(),
            sessions: SessionConfig::default(),
            folders: Vec::new(),
            tunnels: Vec::new(),
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

fn config_path() -> PathBuf { config_dir().join("config.toml") }
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

// ---------------------------------------------------------------------------
// Legacy migration
// ---------------------------------------------------------------------------

/// Legacy config format (pre-split). Used only for migration detection.
#[derive(Deserialize)]
struct LegacyConfig {
    #[serde(default)]
    general: Option<LegacyGeneral>,
    #[serde(default)]
    layout: Option<LayoutConfig>,
    #[serde(default)]
    sessions: Option<SessionConfig>,
    #[serde(default)]
    folders: Option<Vec<ServerFolder>>,
    #[serde(default)]
    tunnels: Option<Vec<SavedTunnel>>,
}

#[derive(Deserialize)]
struct LegacyGeneral {
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    font_size: Option<f32>,
    #[serde(default)]
    font_name: Option<String>,
    // terminal_background and terminal_foreground are ignored —
    // they are now derived from the color scheme.
}

/// Detect a legacy single-file config with `[general]` and migrate to split files.
///
/// Backs up the old file as `config.toml.bak`.
pub fn migrate_if_needed() {
    let path = config_path();
    if !path.exists() { return; }
    // Don't migrate if state.toml already exists (already migrated).
    if state_path().exists() { return; }

    let Ok(contents) = fs::read_to_string(&path) else { return; };

    // Detect legacy format by presence of `[general]` section.
    if !contents.contains("[general]") { return; }

    let Ok(legacy) = toml::from_str::<LegacyConfig>(&contents) else { return; };

    log::info!("Migrating legacy config.toml to split config + state");

    // Build UserConfig from legacy.
    let mut user_config = UserConfig::default();
    if let Some(general) = &legacy.general {
        if let Some(theme) = &general.theme {
            user_config.colors.theme = theme.to_lowercase();
        }
        if let Some(size) = general.font_size {
            user_config.font.size = size;
        }
        if let Some(name) = &general.font_name {
            user_config.font.normal.family = name.clone();
        }
    }

    // Build PersistentState from legacy.
    let persistent = PersistentState {
        layout: legacy.layout.unwrap_or_default(),
        sessions: legacy.sessions.unwrap_or_default(),
        folders: legacy.folders.unwrap_or_default(),
        tunnels: legacy.tunnels.unwrap_or_default(),
    };

    // Back up old config.
    let bak = path.with_extension("toml.bak");
    if let Err(e) = fs::copy(&path, &bak) {
        log::warn!("Failed to back up old config: {e}");
    }

    // Write split files.
    if let Err(e) = save_user_config(&user_config) {
        log::error!("Failed to write new config.toml: {e}");
    }
    if let Err(e) = save_persistent_state(&persistent) {
        log::error!("Failed to write state.toml: {e}");
    }

    log::info!("Migration complete. Old config backed up to {}", bak.display());
}
