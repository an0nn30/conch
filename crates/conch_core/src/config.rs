//! Configuration and persistent state management.
//!
//! Split into three files:
//! - `config.toml` — terminal + appearance prefs (Alacritty-compatible + [conch.*] extensions)
//! - `sessions.toml` — server folders + tunnels (user data, app-managed)
//! - `state.toml` — ephemeral UI state (not user-edited)
//!
//! Legacy single-file `config.toml` with `[general]` section is automatically migrated (v1→v2).
//! Two-file layout with `[keyboard]`/`[session]` at top level is migrated (v2→v3).

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
    pub window: WindowConfig,
    #[serde(default)]
    pub font: FontConfig,
    #[serde(default)]
    pub colors: ColorsConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub conch: ConchConfig,
}

// ---------------------------------------------------------------------------
// Window config — [window] / [window.dimensions]
// ---------------------------------------------------------------------------

/// Window decoration style (mirrors Alacritty `window.decorations`).
///
/// - `Full` — normal title bar and borders (default)
/// - `Transparent` — transparent title bar, content extends behind it (macOS only)
/// - `Buttonless` — transparent title bar with no window buttons (macOS only)
/// - `None` — no title bar or borders
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub enum WindowDecorations {
    #[default]
    Full,
    Transparent,
    Buttonless,
    None,
}

/// Case-insensitive deserialization for WindowDecorations.
/// Accepts "Full", "full", "FULL", etc.
impl<'de> serde::Deserialize<'de> for WindowDecorations {
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

/// Window configuration (mirrors Alacritty `[window]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    #[serde(default)]
    pub dimensions: WindowDimensions,
    #[serde(default)]
    pub decorations: WindowDecorations,
}

/// Startup window dimensions in character cells (Alacritty `[window.dimensions]`).
///
/// A value of `0` for either field means "use the default".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowDimensions {
    #[serde(default = "default_columns")]
    pub columns: u16,
    #[serde(default = "default_lines")]
    pub lines: u16,
}

fn default_columns() -> u16 { 150 }
fn default_lines() -> u16 { 50 }

impl Default for WindowDimensions {
    fn default() -> Self {
        Self {
            columns: default_columns(),
            lines: default_lines(),
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            dimensions: WindowDimensions::default(),
            decorations: WindowDecorations::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal config — [terminal] / [terminal.shell]
// ---------------------------------------------------------------------------

/// Terminal configuration (mirrors Alacritty `[terminal]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    #[serde(default)]
    pub shell: TerminalShell,
    /// Extra environment variables set in the terminal (Alacritty `[terminal.env]`).
    /// `TERM` and `COLORTERM` are always set unless overridden here.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Cursor appearance (Alacritty `[terminal.cursor]`).
    #[serde(default)]
    pub cursor: CursorConfig,
}

/// Cursor configuration (Alacritty `[terminal.cursor]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorConfig {
    #[serde(default)]
    pub style: CursorStyleConfig,
    /// Cursor style when Vi mode is active. If unset, uses `style`.
    #[serde(default)]
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

/// Cursor style (shape + blinking).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorStyleConfig {
    /// Cursor shape: "Block", "Underline", or "Beam".
    #[serde(default = "default_cursor_shape")]
    pub shape: String,
    /// Whether the cursor blinks.
    /// Accepts boolean (`true`/`false`) or Alacritty strings
    /// (`"Never"`, `"Off"`, `"On"`, `"Always"`).
    #[serde(default = "default_true", deserialize_with = "deserialize_blinking")]
    pub blinking: bool,
}

fn default_cursor_shape() -> String {
    "Block".to_owned()
}

/// Accepts `true`, `false`, `"Never"`, `"Off"`, `"On"`, `"Always"`.
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

impl Default for CursorStyleConfig {
    fn default() -> Self {
        Self {
            shape: default_cursor_shape(),
            blinking: true,
        }
    }
}

/// Shell program and arguments (Alacritty `[terminal.shell]`).
///
/// An empty `program` means "use the default login shell ($SHELL)".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalShell {
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub startup_command: String,
    #[serde(default)]
    pub use_tmux: bool,
}

impl Default for TerminalShell {
    fn default() -> Self {
        Self {
            program: String::new(),
            args: Vec::new(),
            startup_command: String::new(),
            use_tmux: false,
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: TerminalShell::default(),
            env: std::collections::HashMap::new(),
            cursor: CursorConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Conch-specific config — [conch.keyboard] / [conch.ui]
// ---------------------------------------------------------------------------

/// Conch-specific settings namespaced under `[conch]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConchConfig {
    #[serde(default)]
    pub keyboard: KeyboardConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

impl Default for ConchConfig {
    fn default() -> Self {
        Self {
            keyboard: KeyboardConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

/// UI appearance settings (non-terminal).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub font_family: String,
    #[serde(default = "default_ui_size")]
    pub font_size: f32,
    /// Use the native macOS global menu bar instead of an in-window menu bar.
    /// Only applies on macOS; ignored on other platforms. Defaults to false
    /// (menu is rendered in a transparent title bar).
    #[serde(default)]
    pub native_menu_bar: bool,
}

fn default_true() -> bool {
    true
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: default_ui_size(),
            native_menu_bar: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontConfig {
    #[serde(default)]
    pub normal: FontFamily,
    #[serde(default = "default_font_size")]
    pub size: f32,
    /// Extra spacing added to each character cell (Alacritty `[font.offset]`).
    /// `x` adds horizontal pixels per cell, `y` adds vertical pixels.
    #[serde(default)]
    pub offset: FontOffset,
}

/// Extra pixel offset applied to each character cell.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FontOffset {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

impl Default for FontOffset {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
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
    /// UI appearance mode: "dark", "light", or "system".
    #[serde(default = "default_appearance_mode")]
    pub appearance_mode: String,
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
    #[serde(default = "default_focus_plugin_search")]
    pub focus_plugin_search: String,
    #[serde(default = "default_new_window")]
    pub new_window: String,
    #[serde(default = "default_focus_files")]
    pub focus_files: String,
    #[serde(default = "default_zen_mode")]
    pub zen_mode: String,
}

fn default_theme() -> String { "dracula".into() }
fn default_appearance_mode() -> String { "dark".into() }
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
fn default_focus_plugin_search() -> String { "cmd+shift+p".into() }
fn default_new_window() -> String { "cmd+shift+n".into() }
fn default_focus_files() -> String { "cmd+shift+f".into() }
fn default_zen_mode() -> String { "cmd+shift+z".into() }

impl Default for FontFamily {
    fn default() -> Self { Self { family: default_font_name() } }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            normal: FontFamily::default(),
            size: default_font_size(),
            offset: FontOffset::default(),
        }
    }
}

impl Default for ColorsConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            appearance_mode: default_appearance_mode(),
        }
    }
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
            focus_plugin_search: default_focus_plugin_search(),
            new_window: default_new_window(),
            focus_files: default_focus_files(),
            zen_mode: default_zen_mode(),
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
// SessionsConfig — ~/.config/conch/sessions.toml
// ---------------------------------------------------------------------------

/// Server folders and tunnels (user data, app-managed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsConfig {
    #[serde(default)]
    pub folders: Vec<ServerFolder>,
    #[serde(default)]
    pub tunnels: Vec<SavedTunnel>,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            tunnels: Vec::new(),
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutConfig {
    #[serde(default = "default_panel_width")]
    pub left_panel_width: f32,
    #[serde(default)]
    pub left_panel_collapsed: bool,
    #[serde(default)]
    pub right_panel_collapsed: bool,
    /// Persisted window width in logical points (0 = use config default).
    #[serde(default)]
    pub window_width: f32,
    /// Persisted window height in logical points (0 = use config default).
    #[serde(default)]
    pub window_height: f32,
    /// Persisted UI zoom factor (0 or 1.0 = default).
    #[serde(default = "default_zoom")]
    pub zoom_factor: f32,
}

fn default_zoom() -> f32 { 1.0 }

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
            window_width: 0.0,
            window_height: 0.0,
            zoom_factor: 1.0,
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
fn sessions_path() -> PathBuf { config_dir().join("sessions.toml") }

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
// Load / Save — SessionsConfig
// ---------------------------------------------------------------------------

pub fn load_sessions() -> Result<SessionsConfig> {
    let path = sessions_path();
    if !path.exists() {
        log::info!("No sessions.toml at {}, using defaults", path.display());
        return Ok(SessionsConfig::default());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let config: SessionsConfig = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(config)
}

pub fn save_sessions(config: &SessionsConfig) -> Result<()> {
    let dir = config_dir();
    if !dir.exists() { fs::create_dir_all(&dir)?; }
    let contents = toml::to_string_pretty(config).context("Failed to serialize sessions")?;
    fs::write(sessions_path(), contents)?;
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
// Legacy migration — v1 (single file with [general]) → v2 (split)
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
}

/// Detect a legacy single-file config with `[general]` and migrate to split files.
///
/// Backs up the old file as `config.toml.bak`.
fn migrate_v1_to_v2() -> bool {
    let path = config_path();
    if !path.exists() { return false; }
    // Don't migrate if state.toml already exists (already migrated past v1).
    if state_path().exists() { return false; }

    let Ok(contents) = fs::read_to_string(&path) else { return false; };

    // Detect legacy format by presence of `[general]` section.
    if !contents.contains("[general]") { return false; }

    let Ok(legacy) = toml::from_str::<LegacyConfig>(&contents) else { return false; };

    log::info!("Migrating v1 legacy config.toml to v2 split config + state");

    // Build UserConfig from legacy (v2 shape — we'll run v2→v3 migration next).
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

    // Build PersistentState — still has folders/tunnels at this point for v2→v3 to pick up.
    // We write folders/tunnels directly into state.toml as a toml::Value so the v2→v3
    // migration can extract them.
    let mut state_val = toml::value::Table::new();
    if let Some(layout) = &legacy.layout {
        if let Ok(v) = toml::Value::try_from(layout.clone()) {
            state_val.insert("layout".into(), v);
        }
    }
    if let Some(sessions) = &legacy.sessions {
        if let Ok(v) = toml::Value::try_from(sessions.clone()) {
            state_val.insert("sessions".into(), v);
        }
    }
    if let Some(folders) = &legacy.folders {
        if let Ok(v) = toml::Value::try_from(folders.clone()) {
            state_val.insert("folders".into(), v);
        }
    }
    if let Some(tunnels) = &legacy.tunnels {
        if let Ok(v) = toml::Value::try_from(tunnels.clone()) {
            state_val.insert("tunnels".into(), v);
        }
    }

    // Back up old config.
    let bak = path.with_extension("toml.bak");
    if let Err(e) = fs::copy(&path, &bak) {
        log::warn!("Failed to back up old config: {e}");
    }

    // Write split files.
    if let Err(e) = save_user_config(&user_config) {
        log::error!("Failed to write new config.toml: {e}");
    }
    let dir = config_dir();
    if !dir.exists() { let _ = fs::create_dir_all(&dir); }
    let state_str = toml::to_string_pretty(&toml::Value::Table(state_val)).unwrap_or_default();
    if let Err(e) = fs::write(state_path(), &state_str) {
        log::error!("Failed to write state.toml: {e}");
    }

    log::info!("v1→v2 migration complete. Old config backed up to {}", bak.display());
    true
}

// ---------------------------------------------------------------------------
// Migration — v2 (2-file with [keyboard]/[session] at top) → v3 (3-file)
// ---------------------------------------------------------------------------

/// Migrate from v2 (2-file) to v3 (3-file) layout.
///
/// Triggered when `sessions.toml` doesn't exist. Uses `toml::Value` manipulation to:
/// 1. Extract `folders`/`tunnels` from `state.toml` → write `sessions.toml`, rewrite slimmed `state.toml`
/// 2. In `config.toml`: move `[keyboard]` → `[conch.keyboard]`, move `font.ui_family`/`font.ui_size` → `[conch.ui]`,
///    move `[session]` fields → `[terminal.shell]`, back up old config as `.v2.bak`
/// 3. Write empty `sessions.toml` if still missing (marks migration complete)
fn migrate_v2_to_v3() {
    let sessions_file = sessions_path();
    if sessions_file.exists() { return; }

    log::info!("Migrating v2 (2-file) → v3 (3-file) config layout");

    // --- Step 1: Extract folders/tunnels from state.toml ---
    let state_file = state_path();
    let mut sessions_config = SessionsConfig::default();
    if state_file.exists() {
        if let Ok(contents) = fs::read_to_string(&state_file) {
            if let Ok(mut table) = contents.parse::<toml::Value>() {
                if let Some(tbl) = table.as_table_mut() {
                    // Extract folders
                    if let Some(folders_val) = tbl.remove("folders") {
                        if let Ok(folders) = folders_val.try_into::<Vec<ServerFolder>>() {
                            sessions_config.folders = folders;
                        }
                    }
                    // Extract tunnels
                    if let Some(tunnels_val) = tbl.remove("tunnels") {
                        if let Ok(tunnels) = tunnels_val.try_into::<Vec<SavedTunnel>>() {
                            sessions_config.tunnels = tunnels;
                        }
                    }
                    // Rewrite slimmed state.toml
                    let slimmed = toml::to_string_pretty(&table).unwrap_or_default();
                    if let Err(e) = fs::write(&state_file, slimmed) {
                        log::error!("Failed to rewrite state.toml: {e}");
                    }
                }
            }
        }
    }

    // --- Step 2: Restructure config.toml ---
    let config_file = config_path();
    if config_file.exists() {
        if let Ok(contents) = fs::read_to_string(&config_file) {
            if let Ok(mut root) = contents.parse::<toml::Value>() {
                if let Some(tbl) = root.as_table_mut() {
                    let mut conch_tbl = toml::value::Table::new();

                    // Move [keyboard] → [conch.keyboard]
                    if let Some(kb) = tbl.remove("keyboard") {
                        conch_tbl.insert("keyboard".into(), kb);
                    }

                    // Move font.ui_family/ui_size → [conch.ui]
                    let mut ui_tbl = toml::value::Table::new();
                    if let Some(font) = tbl.get_mut("font").and_then(|v| v.as_table_mut()) {
                        if let Some(ui_family) = font.remove("ui_family") {
                            ui_tbl.insert("font_family".into(), ui_family);
                        }
                        if let Some(ui_size) = font.remove("ui_size") {
                            ui_tbl.insert("font_size".into(), ui_size);
                        }
                    }
                    if !ui_tbl.is_empty() {
                        conch_tbl.insert("ui".into(), toml::Value::Table(ui_tbl));
                    }

                    if !conch_tbl.is_empty() {
                        tbl.insert("conch".into(), toml::Value::Table(conch_tbl));
                    }

                    // Move [session] fields → [terminal.shell]
                    if let Some(session) = tbl.remove("session") {
                        if let Some(session_tbl) = session.as_table() {
                            let terminal = tbl
                                .entry("terminal")
                                .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
                            if let Some(term_tbl) = terminal.as_table_mut() {
                                let shell = term_tbl
                                    .entry("shell")
                                    .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
                                if let Some(shell_tbl) = shell.as_table_mut() {
                                    // Move startup_command and use_tmux (skip `shell` — it was redundant with program)
                                    if let Some(v) = session_tbl.get("startup_command") {
                                        shell_tbl.insert("startup_command".into(), v.clone());
                                    }
                                    if let Some(v) = session_tbl.get("use_tmux") {
                                        shell_tbl.insert("use_tmux".into(), v.clone());
                                    }
                                    // If [session].shell was set and terminal.shell.program is empty, migrate it
                                    if let Some(toml::Value::String(old_shell)) = session_tbl.get("shell") {
                                        if !old_shell.is_empty() {
                                            let program = shell_tbl
                                                .get("program")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            if program.is_empty() {
                                                shell_tbl.insert(
                                                    "program".into(),
                                                    toml::Value::String(old_shell.clone()),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Back up old config as .v2.bak
                    let bak = config_file.with_extension("toml.v2.bak");
                    if let Err(e) = fs::copy(&config_file, &bak) {
                        log::warn!("Failed to back up v2 config: {e}");
                    }

                    // Write restructured config.toml
                    let restructured = toml::to_string_pretty(&root).unwrap_or_default();
                    if let Err(e) = fs::write(&config_file, restructured) {
                        log::error!("Failed to write restructured config.toml: {e}");
                    }
                }
            }
        }
    }

    // --- Step 3: Write sessions.toml (marks migration complete) ---
    if let Err(e) = save_sessions(&sessions_config) {
        log::error!("Failed to write sessions.toml: {e}");
    }

    log::info!("v2→v3 migration complete");
}

// ---------------------------------------------------------------------------
// Public migration entry point
// ---------------------------------------------------------------------------

/// Run all necessary migrations in order.
pub fn migrate_if_needed() {
    migrate_v1_to_v2();
    migrate_v2_to_v3();
}
