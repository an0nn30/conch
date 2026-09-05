//! Configuration and persistent state management.
//!
//! Split into two files on disk:
//! - `config.toml` — terminal + appearance prefs (Alacritty-compatible + [termlab.*] extensions)
//! - `state.toml` — ephemeral UI state (not user-edited)

mod colors;
mod editor;
mod font;
mod persistent;
mod terminal;
mod termlab;
mod window;

pub use colors::*;
pub use editor::*;
pub use font::*;
pub use persistent::*;
pub use terminal::*;
pub use termlab::*;
pub use window::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Atomic write utility
// ---------------------------------------------------------------------------

/// Build a temp path for `path` that no concurrent writer of the same target
/// can collide on: a fixed name (the old `path.with_extension("tmp")`) is
/// shared by every caller, so two writers racing `atomic_write` for the same
/// target can interleave one's truncate with the other's `write_all` and
/// publish a torn file on rename. `state.toml` now has cross-thread writers
/// (the panel-host bounds debounce thread alongside main-thread saves — see
/// the branch review's F2), so belt-and-braces this alongside the
/// `update_persistent_state` lock: a process id + monotonic counter suffix
/// makes every call's temp file its own, even for two writers targeting the
/// same path in the same process.
fn unique_temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("state.toml");
    path.with_file_name(format!(".{file_name}.{pid}.{n}.tmp"))
}

/// Write data to a file atomically: write to a temporary file first,
/// then rename to the target path. This prevents corruption from
/// partial writes due to crashes or power loss.
///
/// If the target file already exists, its permissions are copied to the
/// temporary file before the rename so that restricted modes (e.g. 0600)
/// are preserved.
///
/// The temp file's name is unique per call (see [`unique_temp_path`]) so
/// concurrent writers targeting the same `path` never share one temp file.
/// That is a hardening measure, not the primary fix for state.toml's
/// load-mutate-save race — [`update_persistent_state`] serializing the
/// whole span is.
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = unique_temp_path(path);
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

/// Guards the load-mutate-save critical section for `state.toml`. Every
/// writer of persisted state — `save_window_layout`, `save_window_metrics`,
/// `set_zoom_level`, `persist_enabled_plugins`, `persist_chooser_size`, and
/// the panel-host bounds debounce thread's `persist_tool_window_bounds` — go
/// through [`update_persistent_state`], which holds this lock across its own
/// load + mutate + save. Without it, two writers on different threads (the
/// bounds debounce thread is the new one) can each load a stale snapshot and
/// have one's mutation silently dropped when the other's save lands after,
/// or — combined with `atomic_write`'s old fixed temp-file name — publish a
/// torn file that fails to parse, which every call site's
/// `unwrap_or_default()` then turns into a silent full-state wipe. See the
/// branch review's F2.
///
/// A plain `Mutex<()>` guards the *file*, not any in-memory value, so `()`
/// is the right payload. Poisoning is recovered rather than propagated: a
/// panic mid-save should not brick every later save in the process.
static STATE_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn load_persistent_state() -> Result<PersistentState> {
    load_persistent_state_at(&state_path())
}

fn load_persistent_state_at(path: &Path) -> Result<PersistentState> {
    if !path.exists() {
        log::info!("No state.toml at {}, using defaults", path.display());
        return Ok(PersistentState::default());
    }
    let contents =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let state: PersistentState =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(state)
}

pub fn save_persistent_state(state: &PersistentState) -> Result<()> {
    save_persistent_state_at(&state_path(), state)
}

fn save_persistent_state_at(path: &Path, state: &PersistentState) -> Result<()> {
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        fs::create_dir_all(dir)?;
    }
    let contents = toml::to_string_pretty(state).context("Failed to serialize state")?;
    atomic_write(path, contents.as_bytes()).context("Failed to write state.toml atomically")?;
    Ok(())
}

/// Load the current persistent state, apply `mutator` to it, and save the
/// result — all under [`STATE_WRITE_LOCK`] so no other writer's
/// load-mutate-save can interleave with this one. This is THE way to modify
/// `state.toml`; a hand-rolled `load_persistent_state()` + mutate +
/// `save_persistent_state()` at a call site is exactly the unlocked race
/// F2 describes.
///
/// `mutator` returns whether the state actually changed. Returning `false`
/// skips the save entirely (e.g. `save_window_metrics`'s "steady state — no
/// write on every launch" check) — that decision now happens *inside* the
/// lock, against the freshest load, rather than against a pre-lock read that
/// could itself be stale.
pub fn update_persistent_state<F>(mutator: F) -> Result<()>
where
    F: FnOnce(&mut PersistentState) -> bool,
{
    update_persistent_state_at(&state_path(), mutator)
}

fn update_persistent_state_at<F>(path: &Path, mutator: F) -> Result<()>
where
    F: FnOnce(&mut PersistentState) -> bool,
{
    let _guard = STATE_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = load_persistent_state_at(path).unwrap_or_default();
    if !mutator(&mut state) {
        return Ok(());
    }
    save_persistent_state_at(path, &state)
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

    // -------------------------------------------------------------------
    // F2 (branch review, 2026-08-19-popout-tool-windows): state.toml
    // concurrent load-mutate-save. `unique_temp_path` is the belt
    // (atomic_write's own hardening); `update_persistent_state` is the
    // braces (the actual fix — one lock around the whole load-mutate-save
    // span for every writer).
    // -------------------------------------------------------------------

    #[test]
    fn unique_temp_path_differs_across_calls_for_the_same_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let a = unique_temp_path(&path);
        let b = unique_temp_path(&path);
        assert_ne!(
            a, b,
            "two calls targeting the same file must not collide on one shared tmp path \
             (the old path.with_extension(\"tmp\") did exactly that)"
        );
        assert_eq!(
            a.parent(),
            Some(dir.path()),
            "the temp file lives alongside its target, same as before"
        );
    }

    #[test]
    fn atomic_write_concurrent_writers_never_produce_a_torn_file() {
        // Below update_persistent_state's lock: this isolates the
        // unique-temp-name hardening in atomic_write itself. Many threads
        // hammer the SAME target path with distinct, non-overlapping
        // contents; every rename must publish one writer's complete
        // content, never a mix of two.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.toml");
        let payloads: Vec<String> = (0..8)
            .map(|i| format!("{}\n", "x".repeat(37 + i)))
            .collect();

        let handles: Vec<_> = payloads
            .iter()
            .cloned()
            .map(|payload| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        atomic_write(&path, payload.as_bytes()).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let final_contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            payloads.contains(&final_contents),
            "the published file must be exactly one writer's complete payload, never a torn mix"
        );
    }

    #[test]
    fn update_persistent_state_serializes_concurrent_writers() {
        // Above the lock: many threads on distinct OS threads (the shape
        // introduced by the panel-host bounds debounce thread) each run a
        // full load-mutate-save through update_persistent_state_at. If the
        // lock did not span the whole load-mutate-save, this drops entries
        // (lost update) whenever two threads' loads race each other.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        const WRITERS: usize = 12;
        const WRITES_EACH: usize = 8;

        let handles: Vec<_> = (0..WRITERS)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for j in 0..WRITES_EACH {
                        let tag = format!("writer-{i}-{j}");
                        update_persistent_state_at(&path, move |state| {
                            state.loaded_plugins.push(tag);
                            true
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        // A torn write would fail to parse here at all; a lost update would
        // parse fine but come up short.
        let final_state = load_persistent_state_at(&path).unwrap();
        assert_eq!(
            final_state.loaded_plugins.len(),
            WRITERS * WRITES_EACH,
            "every writer's mutation must survive — a lost update silently drops entries"
        );
        let unique: std::collections::HashSet<_> = final_state.loaded_plugins.iter().collect();
        assert_eq!(
            unique.len(),
            WRITERS * WRITES_EACH,
            "no entry was duplicated or corrupted into another's"
        );
    }

    #[test]
    fn update_persistent_state_skips_save_when_mutator_reports_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        // Seed a file, then run a no-op mutator (returns false) and confirm
        // nothing was rewritten — the save_window_metrics "steady state, no
        // write on every launch" path this preserves.
        update_persistent_state_at(&path, |state| {
            state.loaded_plugins.push("seed".into());
            true
        })
        .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        update_persistent_state_at(&path, |_state| false).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            before, after,
            "mutator returning false must skip the save entirely"
        );
    }

    #[test]
    fn old_shaped_state_toml_loads_through_the_real_disk_path_with_project_fields_defaulted() {
        // (fix round 1, F9) The persistent.rs back-compat test parses a TOML
        // string directly with `toml::from_str`. That proves serde's
        // `#[serde(default)]` contract but says nothing about the REAL path
        // an old install's file actually goes through — file existence
        // checks, `load_persistent_state_at`'s read, `update_persistent_state_at`'s
        // load-mutate-save cycle. This exercises that path end to end
        // against an old-shaped file genuinely sitting on disk.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(
            &path,
            b"loaded_plugins = [\"my-plugin\"]\n\n[layout]\nzoom_factor = 1.5\n",
        )
        .unwrap();

        let state = load_persistent_state_at(&path).unwrap();
        assert!(state.recent_projects.is_empty());
        assert!(state.project_layouts.is_empty());
        assert_eq!(
            state.layout.zoom_factor, 1.5,
            "the existing keys still load"
        );

        // The real update path too: a project-mode-shaped mutation lands
        // correctly, and the pre-existing fields survive the round trip.
        update_persistent_state_at(&path, |s| {
            s.recent_projects.push(RecentProject {
                path: "/repo".into(),
                last_opened_ms: 9,
            });
            true
        })
        .unwrap();
        let reloaded = load_persistent_state_at(&path).unwrap();
        assert_eq!(reloaded.recent_projects.len(), 1);
        assert_eq!(reloaded.recent_projects[0].path, "/repo");
        assert_eq!(
            reloaded.layout.zoom_factor, 1.5,
            "a field this update never touched survives the load-mutate-save round trip"
        );
        assert_eq!(reloaded.loaded_plugins, vec!["my-plugin".to_string()]);
    }

    #[test]
    fn editor_vim_mode_reads_from_the_editor_table() {
        let cfg: UserConfig = toml::from_str("[editor]\nvim_mode = true").unwrap();
        assert!(cfg.editor.vim_mode);
    }

    #[test]
    fn old_editor_config_gets_lsp_defaults() {
        let cfg: UserConfig = toml::from_str("[editor]\nvim_mode = true\n").unwrap();
        assert!(cfg.editor.lsp.enabled);
        assert!(cfg.editor.lsp.languages.typescript);
        assert!(cfg.editor.lsp.languages.json);
        assert!(cfg.editor.lsp.languages.python);
        assert!(cfg.editor.lsp.languages.rust);
        assert!(cfg.editor.lsp.languages.go);
        assert!(cfg.editor.lsp.languages.clangd);
        assert!(cfg.editor.lsp.languages.java);
        assert_eq!(cfg.termlab.keyboard.editor_completion, "ctrl+space");
        assert_eq!(cfg.termlab.keyboard.editor_go_to_definition, "f12");
    }

    #[test]
    fn explicit_lsp_language_disables_survive_a_round_trip() {
        let cfg: UserConfig = toml::from_str(
            "[editor.lsp]\nenabled = false\nsuggestions_while_typing = false\n\n[editor.lsp.languages]\ntypescript = false\njava = false\n",
        )
        .unwrap();
        let serialized = toml::to_string(&cfg).unwrap();
        let round_tripped: UserConfig = toml::from_str(&serialized).unwrap();

        assert!(!round_tripped.editor.lsp.enabled);
        assert!(!round_tripped.editor.lsp.suggestions_while_typing);
        assert!(!round_tripped.editor.lsp.languages.typescript);
        assert!(!round_tripped.editor.lsp.languages.java);
        assert!(round_tripped.editor.lsp.languages.rust);
    }

    #[test]
    fn editor_defaults_off_when_the_section_is_absent() {
        // Every config.toml written before this section existed.
        let cfg: UserConfig = toml::from_str("").unwrap();
        assert!(!cfg.editor.vim_mode);
        assert!(!UserConfig::default().editor.vim_mode);
    }

    #[test]
    fn search_in_project_defaults_to_cmd_shift_f_and_survives_an_override() {
        let cfg = UserConfig::default();
        assert_eq!(cfg.termlab.keyboard.search_in_project, "cmd+shift+f");
        let overridden: UserConfig =
            toml::from_str("[termlab.keyboard]\nsearch_in_project = \"ctrl+alt+f\"\n")
                .expect("parse");
        assert_eq!(overridden.termlab.keyboard.search_in_project, "ctrl+alt+f");
        // Back-compat: a config written before this key existed still parses.
        let legacy: UserConfig =
            toml::from_str("[termlab.keyboard]\nnew_tab = \"cmd+t\"\n").expect("parse");
        assert_eq!(legacy.termlab.keyboard.search_in_project, "cmd+shift+f");
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
