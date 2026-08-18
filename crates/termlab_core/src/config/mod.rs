//! Configuration and persistent state management.
//!
//! Split into two files on disk:
//! - `config.toml` — terminal + appearance prefs (Alacritty-compatible + [termlab.*] extensions)
//! - `state.toml` — ephemeral UI state (not user-edited)

mod colors;
mod editor;
mod termlab;
mod font;
mod persistent;
mod terminal;
mod window;

pub use colors::*;
pub use editor::*;
pub use termlab::*;
pub use font::*;
pub use persistent::*;
pub use terminal::*;
pub use window::*;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Atomic write utility
// ---------------------------------------------------------------------------

/// Write data to a file atomically: write to a temporary file first,
/// then rename to the target path. This prevents corruption from
/// partial writes due to crashes or power loss.
///
/// If the target file already exists, its permissions are copied to the
/// temporary file before the rename so that restricted modes (e.g. 0600)
/// are preserved.
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;

    // Preserve permissions from the existing file if present.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
    }

    fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// UserConfig — ~/.config/termlab/config.toml
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
    #[serde(default, skip_serializing)]
    pub font: FontConfig,
    pub colors: ColorsConfig,
    pub terminal: TerminalConfig,
    pub editor: EditorConfig,
    /// Accepts the legacy `[conch.*]` section name from pre-rebrand configs.
    #[serde(alias = "conch")]
    pub termlab: TermLabConfig,
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
            editor: EditorConfig::default(),
            termlab: TermLabConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Returns the config directory.
///
/// - macOS / Linux: `~/.config/termlab/`
/// - Windows: `%APPDATA%\termlab\`
pub fn config_dir() -> PathBuf {
    #[cfg(not(target_os = "windows"))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".config")
            .join("termlab")
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("termlab")
    }
}

/// Legacy (pre-rebrand) config directory: `~/.config/conch/` / `%APPDATA%\conch\`.
fn legacy_config_dir() -> PathBuf {
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

/// One-time migration of the legacy `conch` config directory to `termlab`.
///
/// Runs before anything reads [`config_dir`]. Two steps:
///
/// 1. The IntelliJ-era TermLab app used the same `termlab` directory name
///    with incompatible file formats (its own `vault.enc`, `ssh-hosts.json`,
///    `tunnels.json`). If the target dir holds that app's data, archive it to
///    `termlab-jvm-backup` first — the two apps must never share files.
/// 2. If the target is then free and a legacy `conch` dir exists, move it
///    (config, state, themes, vault, plugins) wholesale.
///
/// Every failure path leaves directories untouched so the app still starts —
/// with default settings — rather than failing or corrupting either data set.
pub fn migrate_legacy_config_dir() {
    migrate_config_dirs(&legacy_config_dir(), &config_dir());
}

fn migrate_config_dirs(old_dir: &std::path::Path, new_dir: &std::path::Path) {
    // Java-era TermLab data: has its stores, lacks this app's config.toml.
    let target_is_jvm_data = new_dir.exists()
        && new_dir.join("ssh-hosts.json").exists()
        && !new_dir.join("config.toml").exists();
    if target_is_jvm_data {
        let backup = new_dir.with_file_name("termlab-jvm-backup");
        if backup.exists() {
            log::warn!(
                "Config dir {} holds IntelliJ TermLab data but backup {} already exists; leaving both untouched",
                new_dir.display(),
                backup.display()
            );
            return;
        }
        match fs::rename(new_dir, &backup) {
            Ok(()) => log::info!(
                "Archived IntelliJ TermLab data {} -> {}",
                new_dir.display(),
                backup.display()
            ),
            Err(e) => {
                log::warn!(
                    "Could not archive IntelliJ TermLab data {}: {e}; leaving untouched",
                    new_dir.display()
                );
                return;
            }
        }
    }

    if new_dir.exists() || !old_dir.exists() {
        return;
    }
    match fs::rename(old_dir, new_dir) {
        Ok(()) => log::info!(
            "Migrated legacy config dir {} -> {}",
            old_dir.display(),
            new_dir.display()
        ),
        Err(e) => log::warn!(
            "Could not migrate legacy config dir {} -> {}: {e}",
            old_dir.display(),
            new_dir.display()
        ),
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}
fn state_path() -> PathBuf {
    config_dir().join("state.toml")
}

// ---------------------------------------------------------------------------
// Load / Save — UserConfig
// ---------------------------------------------------------------------------

pub fn load_user_config() -> Result<UserConfig> {
    let path = config_path();
    if !path.exists() {
        log::info!("No config.toml at {}, using defaults", path.display());
        return Ok(UserConfig::default());
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let config: UserConfig =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(config)
}

pub fn save_user_config(config: &UserConfig) -> Result<()> {
    let dir = config_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    let contents = toml::to_string_pretty(config).context("Failed to serialize config")?;
    atomic_write(&config_path(), contents.as_bytes())
        .context("Failed to write config.toml atomically")?;
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
    let contents =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let state: PersistentState =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(state)
}

pub fn save_persistent_state(state: &PersistentState) -> Result<()> {
    let dir = config_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    let contents = toml::to_string_pretty(state).context("Failed to serialize state")?;
    atomic_write(&state_path(), contents.as_bytes())
        .context("Failed to write state.toml atomically")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &std::path::Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn migrate_moves_legacy_dir_when_target_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("conch");
        let new = tmp.path().join("termlab");
        touch(&old.join("config.toml"));
        migrate_config_dirs(&old, &new);
        assert!(!old.exists());
        assert!(new.join("config.toml").exists());
    }

    #[test]
    fn migrate_archives_jvm_data_then_moves_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("conch");
        let new = tmp.path().join("termlab");
        touch(&old.join("vault.enc"));
        touch(&new.join("ssh-hosts.json"));
        touch(&new.join("vault.enc"));
        migrate_config_dirs(&old, &new);
        let backup = tmp.path().join("termlab-jvm-backup");
        assert!(backup.join("ssh-hosts.json").exists(), "jvm data archived");
        assert!(new.join("vault.enc").exists(), "conch data moved in");
        assert!(!old.exists());
    }

    #[test]
    fn migrate_leaves_rust_owned_target_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("conch");
        let new = tmp.path().join("termlab");
        touch(&old.join("config.toml"));
        touch(&new.join("config.toml"));
        touch(&new.join("ssh-hosts.json"));
        migrate_config_dirs(&old, &new);
        assert!(old.exists(), "legacy dir untouched when target is owned");
        assert!(!tmp.path().join("termlab-jvm-backup").exists());
    }

    #[test]
    fn migrate_refuses_second_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("conch");
        let new = tmp.path().join("termlab");
        touch(&old.join("config.toml"));
        touch(&new.join("ssh-hosts.json"));
        fs::create_dir_all(tmp.path().join("termlab-jvm-backup")).unwrap();
        migrate_config_dirs(&old, &new);
        assert!(new.join("ssh-hosts.json").exists(), "nothing moved");
        assert!(old.exists());
    }

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
        assert_eq!(
            cfg.resolved_terminal_font().size,
            FontConfig::default().size
        );
    }

    #[test]
    fn serialized_config_omits_legacy_font_section() {
        let config = UserConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        // The legacy [font] section should not appear in serialized output.
        // Only [terminal.font] should be present.
        assert!(
            !toml_str.contains("\n[font]\n"),
            "Legacy [font] section should not appear in serialized output, got:\n{toml_str}"
        );
    }

    #[test]
    fn atomic_write_creates_file_with_correct_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.toml");
        atomic_write(&path, b"hello world").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn atomic_write_leaves_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        atomic_write(&path, b"{\"key\": \"value\"}").unwrap();
        let tmp = path.with_extension("tmp");
        assert!(
            !tmp.exists(),
            ".tmp file should not remain after successful write"
        );
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "old content").unwrap();
        atomic_write(&path, b"new content").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
    }

    #[test]
    fn atomic_write_empty_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.toml");
        atomic_write(&path, b"").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn editor_vim_mode_reads_from_the_editor_table() {
        let cfg: UserConfig = toml::from_str("[editor]\nvim_mode = true").unwrap();
        assert!(cfg.editor.vim_mode);
    }

    #[test]
    fn editor_defaults_off_when_the_section_is_absent() {
        // Every config.toml written before this section existed.
        let cfg: UserConfig = toml::from_str("").unwrap();
        assert!(!cfg.editor.vim_mode);
        assert!(!UserConfig::default().editor.vim_mode);
    }

    #[test]
    fn ui_font_from_full_config() {
        let toml_str = r#"
            [termlab.ui.font]
            small = 10.0
            list = 12.0
            normal = 13.0
        "#;
        let cfg: UserConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.termlab.ui.font.small, 10.0);
        assert_eq!(cfg.termlab.ui.font.list, 12.0);
        assert_eq!(cfg.termlab.ui.font.normal, 13.0);
    }
}
