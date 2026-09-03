//! Alacritty-compatible color theme loading.
//!
//! Deserializes unmodified Alacritty `.toml` theme files (e.g. gruvbox_dark.toml,
//! catppuccin_mocha.toml) and provides a built-in fallback palette — the same
//! one bundled as `frontend/themes/TermLab Dark.toml` (see [`ColorScheme`]'s
//! `Default` impl).
//!
//! ## Compatibility contract
//!
//! [`ColorScheme`] accepts the FULL current Alacritty `[colors]` schema (see
//! the `alacritty.5.scd` reference: `primary.{background,foreground,
//! dim_foreground,bright_foreground}`, `cursor`, `vi_mode_cursor`,
//! `selection`, `search.{matches,focused_match}`, `hints.{start,end}`,
//! `line_indicator`, `footer_bar`, `normal`, `bright`, `dim`,
//! `indexed_colors[]`, `transparent_background_colors`,
//! `draw_bold_text_with_bright_colors`) so that any valid Alacritty theme
//! file parses without error. Every color-bearing field is stored as a raw
//! `String`, not a validated/typed RGB value, which means both color string
//! forms Alacritty accepts parse for free: `#rrggbb` and the legacy
//! `0x`-prefixed form, as well as the `CellRgb` sentinel strings
//! (`"CellForeground"` / `"CellBackground"`) used by `cursor`,
//! `vi_mode_cursor`, `selection`, `search`, and `hints`. Unknown keys beyond
//! this schema are tolerated too, since nothing here (or upstream `toml`/
//! `serde`) sets `#[serde(deny_unknown_fields)]`.
//!
//! Those raw forms are canonicalized to a single `#rrggbb` form by
//! [`crate::color_normalize::normalize_scheme`], called from [`load_theme`] —
//! the one boundary in the crate where that happens. Everything downstream
//! (`termlab_tauri::theme`'s hex helpers, [`PalettePreview`], the theme
//! catalog) therefore only ever sees `#`-form colors.
//!
//! Only a subset of the accepted schema currently has terminal meaning and
//! is APPLIED further down the pipeline (`termlab_tauri::theme`): `primary`,
//! `cursor`, `selection`, `normal`, `bright`, and `indexed_colors` (carried
//! into xterm's `ITheme.extendedAnsi` — see
//! `termlab_tauri::extended_ansi`). `dim` is parsed and preserved but has no
//! xterm carrier at all: xterm 5.5.0's `ITheme` has no dim palette and its
//! renderers derive dim by halving the opacity of the already-chosen color
//! (the `xterm-dim` class), so there is nothing to hand it. `vi_mode_cursor`,
//! `search`, `hints`, `line_indicator`, `footer_bar`,
//! `transparent_background_colors`, and `draw_bold_text_with_bright_colors`
//! are parsed and otherwise ignored by design (no terminal-chrome meaning
//! here today).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level wrapper matching Alacritty theme file structure.
#[derive(Debug, Clone, Deserialize)]
pub struct AlacrittyThemeFile {
    pub colors: ColorScheme,
}

/// Full color scheme with primary, normal, bright, and every optional
/// section the current Alacritty schema defines. See the module docs for
/// which of these are applied vs. parsed-and-ignored today.
#[derive(Debug, Clone, Deserialize)]
pub struct ColorScheme {
    pub primary: PrimaryColors,
    pub normal: AnsiColors,
    pub bright: AnsiColors,
    #[serde(default)]
    pub dim: Option<AnsiColors>,
    #[serde(default)]
    pub cursor: Option<CursorColors>,
    /// Cursor colors used while vi mode is active. Parsed, not applied
    /// (TermLab has no vi mode today).
    #[serde(default)]
    pub vi_mode_cursor: Option<CursorColors>,
    #[serde(default)]
    pub selection: Option<SelectionColors>,
    /// `colors.search.{matches,focused_match}`. Parsed, not applied
    /// (TermLab's terminal search UI doesn't yet read theme colors).
    #[serde(default)]
    pub search: Option<SearchColors>,
    /// `colors.hints.{start,end}` (keyboard hint/regex-match labels).
    /// Parsed, not applied.
    #[serde(default)]
    pub hints: Option<HintColors>,
    /// Position-in-history indicator shown during search/vi mode. Parsed,
    /// not applied.
    #[serde(default)]
    pub line_indicator: Option<LineIndicatorColors>,
    /// Footer bar (search input, hyperlink preview, etc). Parsed, not
    /// applied.
    #[serde(default)]
    pub footer_bar: Option<FooterBarColors>,
    /// Extended-palette overrides for indices 16-255. Parsed and preserved,
    /// not yet carried into the xterm extended palette — see module docs.
    #[serde(default)]
    pub indexed_colors: Vec<IndexedColor>,
    #[serde(default)]
    pub transparent_background_colors: Option<bool>,
    #[serde(default)]
    pub draw_bold_text_with_bright_colors: Option<bool>,
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

/// `text`/`cursor` accept `#rrggbb`, `0xrrggbb`, or the `CellForeground`/
/// `CellBackground` sentinel strings — all are just strings here, see the
/// module docs.
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

/// `colors.search`. Both sub-tables are individually optional in real
/// theme files (a theme may set only `matches`, only `focused_match`, or
/// neither).
#[derive(Debug, Clone, Deserialize)]
pub struct SearchColors {
    #[serde(default)]
    pub matches: Option<SearchMatchColors>,
    #[serde(default)]
    pub focused_match: Option<SearchMatchColors>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchMatchColors {
    pub foreground: String,
    pub background: String,
}

/// `colors.hints.{start,end}`.
#[derive(Debug, Clone, Deserialize)]
pub struct HintColors {
    #[serde(default)]
    pub start: Option<HintColorPair>,
    #[serde(default)]
    pub end: Option<HintColorPair>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HintColorPair {
    pub foreground: String,
    pub background: String,
}

/// `colors.line_indicator`. Both fields are individually optional upstream
/// (the Alacritty default is `{ foreground = "None", background = "None" }`,
/// i.e. omitted).
#[derive(Debug, Clone, Deserialize)]
pub struct LineIndicatorColors {
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
}

/// `colors.footer_bar`.
#[derive(Debug, Clone, Deserialize)]
pub struct FooterBarColors {
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
}

/// One entry of `colors.indexed_colors[]`: `{ index = 16..=255, color =
/// "<string>" }`.
#[derive(Debug, Clone, Deserialize)]
pub struct IndexedColor {
    pub index: u8,
    pub color: String,
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
    /// TermLab Dark — the app's default terminal palette, kept byte-identical
    /// to the bundled `frontend/themes/TermLab Dark.toml`.
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
            vi_mode_cursor: None,
            selection: Some(SelectionColors {
                text: "#f8f8f2".into(),
                background: "#44475a".into(),
            }),
            search: None,
            hints: None,
            line_indicator: None,
            footer_bar: None,
            indexed_colors: Vec::new(),
            transparent_background_colors: None,
            draw_bold_text_with_bright_colors: None,
        }
    }
}

/// Host-injected override for [`bundled_themes_dir`], set at most once via
/// [`set_bundled_themes_dir`]. `termlab_core` has no Tauri dependency, so it
/// cannot resolve a packaged app's real resource directory itself — the
/// Tauri host injects it at startup instead. See [`set_bundled_themes_dir`]
/// for the full contract.
static BUNDLED_THEMES_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Inject the real bundled-themes directory resolved by the host
/// application (e.g. Tauri's `resource_dir()` in a packaged build),
/// overriding the dev-only [`bundled_themes_dir`] fallback below.
///
/// Intended to be called exactly once, early at startup, before any theme
/// is resolved — `termlab_tauri`'s `tauri::Builder::setup` does this (see
/// `termlab_tauri::bundled_themes::inject_bundled_themes_dir`). Returns
/// `true` if this call set the override, `false` if an override was already
/// set. A duplicate call is treated as a no-op rather than a panic: the app
/// only ever calls this once in practice, and a defensive re-init (or a
/// stray second call from a future refactor) should not crash startup — the
/// first-set value silently wins, matching [`OnceLock::set`]'s semantics.
pub fn set_bundled_themes_dir(path: PathBuf) -> bool {
    BUNDLED_THEMES_DIR_OVERRIDE.set(path).is_ok()
}

/// The dev-only fallback location for the bundled themes directory:
/// `crates/termlab_tauri/frontend/themes/`, resolved at compile time
/// relative to this crate's manifest dir (siblings under `crates/`). Only
/// valid inside a source checkout — see [`bundled_themes_dir`].
fn manifest_relative_themes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("termlab_tauri")
        .join("frontend")
        .join("themes")
}

/// Precedence logic behind [`bundled_themes_dir`], pulled apart into a pure
/// function so the override-vs-fallback rule is unit-testable without
/// touching the process-global [`BUNDLED_THEMES_DIR_OVERRIDE`] (which, once
/// set in a test binary, stays set for every other test sharing that
/// binary — see the `color_scheme` test module for why the "once injected,
/// wins" case is instead proven end-to-end in a separate integration test
/// binary).
fn resolve_bundled_themes_dir(
    override_dir: Option<&Path>,
    dev_fallback: impl FnOnce() -> PathBuf,
) -> PathBuf {
    match override_dir {
        Some(dir) => dir.to_path_buf(),
        None => dev_fallback(),
    }
}

/// Return the bundled themes directory shipped with the frontend.
///
/// Resolves to the host-injected [`BUNDLED_THEMES_DIR_OVERRIDE`] when one has
/// been set via [`set_bundled_themes_dir`] (the packaged-app case — see that
/// function's docs); otherwise falls back to
/// [`manifest_relative_themes_dir`], which only resolves inside a source
/// checkout. Nothing in this crate calls [`set_bundled_themes_dir`], so
/// `termlab_core`'s own dev and test behavior is unaffected by this
/// override — only a host application (`termlab_tauri`) injects it.
fn bundled_themes_dir() -> PathBuf {
    resolve_bundled_themes_dir(
        BUNDLED_THEMES_DIR_OVERRIDE.get().map(PathBuf::as_path),
        manifest_relative_themes_dir,
    )
}

/// Return the user themes directory: `~/.config/termlab/themes/`.
pub fn themes_dir() -> PathBuf {
    crate::config::config_dir().join("themes")
}

/// Load a color scheme from an Alacritty-format TOML file.
///
/// This is the single fallible file → [`ColorScheme`] funnel in the crate,
/// which makes it the one boundary where every accepted color form is
/// canonicalized to `#rrggbb` (see [`crate::color_normalize`]). Both branches
/// of [`resolve_theme_in`] and [`theme_list_entry_for`] go through here, and
/// [`ColorScheme::default`] is already canonical, so no consumer downstream
/// can ever observe a raw `0x`-prefixed or `CellRgb` value.
pub fn load_theme(path: &Path) -> Result<ColorScheme> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read theme from {}", path.display()))?;
    let theme_file: AlacrittyThemeFile = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse theme from {}", path.display()))?;
    let mut colors = theme_file.colors;
    crate::color_normalize::normalize_scheme(&mut colors);
    Ok(colors)
}

/// Scan `dirs` in order and return a map of `name -> path` for every `.toml`
/// file found. Directories earlier in the slice are scanned first; a file
/// with the same stem in a later directory overwrites (wins over) one from
/// an earlier directory, since `HashMap::insert` replaces on duplicate keys.
fn list_themes_in(dirs: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut themes = HashMap::new();
    for dir in dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "toml") {
                    if let Some(stem) = path.file_stem() {
                        themes.insert(stem.to_string_lossy().into_owned(), path);
                    }
                }
            }
        }
    }
    themes
}

/// Scan the bundled themes directory and the user themes directory, returning
/// a map of `name -> path`. The bundled dir is scanned first and the user dir
/// second, so a user theme with the same name overwrites (wins over) a
/// bundled theme of that name.
pub fn list_themes() -> HashMap<String, PathBuf> {
    list_themes_in(&[bundled_themes_dir(), themes_dir()])
}

/// Where a discovered theme file came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSource {
    /// Shipped with the app (`bundled_themes_dir()`).
    Builtin,
    /// Supplied by the user (`themes_dir()`).
    User,
}

/// A small, render-ready palette extracted from a parsed [`ColorScheme`]:
/// background, foreground, and the 16 ANSI colors in standard order (normal
/// black..white at indices 0-7, bright black..white at indices 8-15).
///
/// Colors are copied verbatim from the parsed theme. That is now safe for a
/// literal CSS consumer: [`load_theme`] canonicalizes every accepted color
/// form to `#rrggbb` before a `ColorScheme` escapes it (see
/// [`crate::color_normalize`]), so a `0x`-form or `CellRgb` theme previews
/// with the same colors it renders with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettePreview {
    pub bg: String,
    pub fg: String,
    pub ansi: [String; 16],
}

impl PalettePreview {
    /// Build a preview from an already-parsed theme, reusing the same
    /// `ColorScheme` the rest of the app resolves and applies — there is no
    /// second TOML parsing path for previews.
    pub fn from_scheme(scheme: &ColorScheme) -> Self {
        let normal = scheme.normal.as_array();
        let bright = scheme.bright.as_array();
        let ansi = std::array::from_fn(|i| {
            if i < 8 {
                normal[i].to_string()
            } else {
                bright[i - 8].to_string()
            }
        });
        Self {
            bg: scheme.primary.background.clone(),
            fg: scheme.primary.foreground.clone(),
            ansi,
        }
    }
}

/// One entry in a theme listing: either a theme that parsed successfully
/// (with its preview), or a theme file that failed to parse. Broken files
/// are surfaced rather than silently skipped, so a picker can grey them out
/// with the parse error instead of hiding them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeListEntry {
    Parsed {
        name: String,
        source: ThemeSource,
        // Boxed: PalettePreview's [String; 16] makes it much larger than
        // the other fields here, which would otherwise blow up
        // `size_of::<ThemeListEntry>()` to the size of its largest variant
        // (clippy::large_enum_variant).
        palette_preview: Box<PalettePreview>,
        /// `true` when a user theme of this name shadows (overrides) a
        /// bundled built-in of the same name — the existing later-dirs-win
        /// collision rule (see [`list_themes_in`]), surfaced here rather
        /// than changed.
        shadows_builtin: bool,
    },
    Broken {
        name: String,
        error: String,
    },
}

impl ThemeListEntry {
    pub fn name(&self) -> &str {
        match self {
            ThemeListEntry::Parsed { name, .. } => name,
            ThemeListEntry::Broken { name, .. } => name,
        }
    }
}

fn theme_list_entry_for(
    name: String,
    path: &Path,
    source: ThemeSource,
    shadows_builtin: bool,
) -> ThemeListEntry {
    match load_theme(path) {
        Ok(scheme) => ThemeListEntry::Parsed {
            name,
            source,
            palette_preview: Box::new(PalettePreview::from_scheme(&scheme)),
            shadows_builtin,
        },
        Err(e) => ThemeListEntry::Broken {
            name,
            error: e.to_string(),
        },
    }
}

/// Enumerate every theme discoverable in `bundled_dir` and `user_dir`,
/// layering a [`PalettePreview`] (or a parse error) on top of the existing
/// directory-scan + collision machinery ([`list_themes_in`]).
///
/// A user theme shadows a bundled theme of the same name exactly as
/// `list_themes_in`/`resolve_theme_in` already behave (later dir wins): the
/// shadowed built-in does not get a separate entry, and the surviving user
/// entry carries `shadows_builtin: true`. This applies whether the user
/// theme parses or not — a broken user override still shadows the bundled
/// theme (the same file `resolve_theme_in` would fail to load and silently
/// fall back from), it just surfaces as a `Broken` entry here instead of
/// disappearing.
///
/// Ordering: built-ins first (sorted by name), then user themes (sorted by
/// name).
pub fn list_theme_entries_in(bundled_dir: &Path, user_dir: &Path) -> Vec<ThemeListEntry> {
    let bundled = list_themes_in(&[bundled_dir.to_path_buf()]);
    let user = list_themes_in(&[user_dir.to_path_buf()]);

    let mut builtin_entries: Vec<ThemeListEntry> = bundled
        .iter()
        .filter(|(name, _)| !user.contains_key(*name))
        .map(|(name, path)| theme_list_entry_for(name.clone(), path, ThemeSource::Builtin, false))
        .collect();

    let mut user_entries: Vec<ThemeListEntry> = user
        .iter()
        .map(|(name, path)| {
            let shadows_builtin = bundled.contains_key(name);
            theme_list_entry_for(name.clone(), path, ThemeSource::User, shadows_builtin)
        })
        .collect();

    builtin_entries.sort_by(|a, b| a.name().cmp(b.name()));
    user_entries.sort_by(|a, b| a.name().cmp(b.name()));

    builtin_entries.extend(user_entries);
    builtin_entries
}

/// Enumerate themes against the production directories: the bundled
/// frontend `themes/` dir and `~/.config/termlab/themes/`. Rescanned on
/// every call (no watcher/caching) — see [`list_theme_entries_in`].
pub fn list_theme_entries() -> Vec<ThemeListEntry> {
    list_theme_entries_in(&bundled_themes_dir(), &themes_dir())
}

/// Resolve a theme by name or path: load from disk or fall back to the
/// built-in default palette ([`ColorScheme::default`], i.e. TermLab Dark).
///
/// If `value` is a file path (contains `/`, `\`, or ends with `.toml`), it is
/// loaded directly. A leading `~` is expanded to the home directory.
/// Otherwise `value` is treated as a theme name and looked up in `dirs`, in
/// order (later directories win on name collision — see [`list_themes_in`]).
fn resolve_theme_in(value: &str, dirs: &[PathBuf]) -> ColorScheme {
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
                    "Failed to load theme from '{}': {e}, using the built-in default palette",
                    expanded.display()
                );
            }
        }
    } else {
        let themes = list_themes_in(dirs);
        if let Some(path) = themes.get(value) {
            match load_theme(path) {
                Ok(scheme) => {
                    log::info!("Loaded theme '{}' from {}", value, path.display());
                    return scheme;
                }
                Err(e) => {
                    log::warn!(
                        "Failed to load theme '{}': {e}, using the built-in default palette",
                        value
                    );
                }
            }
        } else {
            // No name is special-cased here any more: the former "dracula"
            // exemption existed only because that name resolved to the
            // hardcoded default without a file behind it. That palette now
            // ships as `TermLab Dark.toml` and resolves through the normal
            // lookup above, so an unmatched name is always worth logging.
            log::info!(
                "Theme '{}' not found in themes dir, using the built-in default palette",
                value
            );
        }
    }
    ColorScheme::default()
}

/// Resolve a theme by name or path against the production theme directories:
/// the bundled frontend `themes/` dir first, then `~/.config/termlab/themes/`
/// (user themes win on name collision). See [`resolve_theme_in`].
pub fn resolve_theme(value: &str) -> ColorScheme {
    resolve_theme_in(value, &[bundled_themes_dir(), themes_dir()])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid Alacritty theme TOML with the given background.
    fn theme_toml(background: &str) -> String {
        format!(
            r##"
[colors.primary]
background = "{background}"
foreground = "#ffffff"
[colors.normal]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"
[colors.bright]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"
"##
        )
    }

    // -----------------------------------------------------------------
    // bundled_themes_dir override (set_bundled_themes_dir)
    // -----------------------------------------------------------------
    //
    // These test `resolve_bundled_themes_dir` — the pure precedence
    // function — rather than calling `set_bundled_themes_dir` on the real
    // `BUNDLED_THEMES_DIR_OVERRIDE`. That static is process-global: once set
    // it stays set for the rest of the test binary, which would make every
    // OTHER test in this file (and in `effective_theme.rs`, compiled into
    // the same `cargo test -p termlab_core` binary) nondeterministically see
    // an injected dir instead of the real dev fallback, depending on test
    // execution order/interleaving. The "injection actually wins, end to
    // end, and is once-only" behavior of the real static is instead proven
    // in `tests/bundled_themes_dir_injection.rs`, a separate integration
    // test binary (its own process) where poisoning the static for the rest
    // of that binary is safe because nothing else in it depends on the
    // fallback.

    #[test]
    fn bundled_themes_dir_prefers_the_injected_override_when_present() {
        let injected = PathBuf::from("/injected/packaged/themes");
        let resolved = resolve_bundled_themes_dir(Some(&injected), manifest_relative_themes_dir);
        assert_eq!(resolved, injected);
    }

    #[test]
    fn bundled_themes_dir_falls_back_to_the_dev_path_when_nothing_injected() {
        let resolved = resolve_bundled_themes_dir(None, manifest_relative_themes_dir);
        assert_eq!(resolved, manifest_relative_themes_dir());
    }

    #[test]
    fn bundled_themes_dir_is_byte_identical_to_the_dev_fallback_when_unset() {
        // Sanity check on the real zero-arg `bundled_themes_dir()`: as long
        // as nothing in this test binary ever calls `set_bundled_themes_dir`
        // (nothing here does), it must keep resolving exactly like it did
        // before this override existed.
        assert_eq!(bundled_themes_dir(), manifest_relative_themes_dir());
        assert!(
            bundled_themes_dir().join("TermLab Dark.toml").is_file(),
            "dev fallback must still find the real bundled theme on disk"
        );
    }

    #[test]
    fn list_themes_finds_bundled_frontend_theme() {
        // Hermetic: use a fake "bundled" dir (a tempdir seeded with a copy of
        // the real generated toml) plus an empty fake user dir, rather than
        // the real production dirs — the real ~/.config/termlab/themes may or
        // may not exist on the machine running this test.
        let bundled = tempfile::tempdir().unwrap();
        std::fs::write(
            bundled.path().join("TermLab Dark.toml"),
            theme_toml("#070A0E"),
        )
        .unwrap();
        let user = tempfile::tempdir().unwrap();

        let themes = list_themes_in(&[bundled.path().to_path_buf(), user.path().to_path_buf()]);
        assert!(
            themes.contains_key("TermLab Dark"),
            "expected 'TermLab Dark' theme to be discoverable, found: {:?}",
            themes.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn resolve_theme_loads_bundled_frontend_theme() {
        let bundled = tempfile::tempdir().unwrap();
        std::fs::write(
            bundled.path().join("TermLab Dark.toml"),
            theme_toml("#070A0E"),
        )
        .unwrap();
        let user = tempfile::tempdir().unwrap();

        let scheme = resolve_theme_in(
            "TermLab Dark",
            &[bundled.path().to_path_buf(), user.path().to_path_buf()],
        );
        assert_eq!(scheme.primary.background, "#070A0E");
    }

    #[test]
    fn resolve_theme_prefers_user_dir_over_bundled_dir_on_name_collision() {
        // Seed a same-named theme in both a fake bundled dir and a fake user
        // dir, with different backgrounds. The user copy must win, per the
        // documented precedence (bundled dir scanned first, user dir second,
        // later insert wins).
        let bundled = tempfile::tempdir().unwrap();
        std::fs::write(bundled.path().join("Shadowed.toml"), theme_toml("#111111")).unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(user.path().join("Shadowed.toml"), theme_toml("#222222")).unwrap();

        let scheme = resolve_theme_in(
            "Shadowed",
            &[bundled.path().to_path_buf(), user.path().to_path_buf()],
        );
        assert_eq!(
            scheme.primary.background, "#222222",
            "user theme dir must take precedence over the bundled theme dir on name collision"
        );

        // Also assert directly on the merged map, which is what production
        // list_themes()/resolve_theme() actually iterate over — this pins
        // the *order* passed to list_themes_in, not just the outcome.
        let themes = list_themes_in(&[bundled.path().to_path_buf(), user.path().to_path_buf()]);
        assert_eq!(
            themes.get("Shadowed").unwrap(),
            &user.path().join("Shadowed.toml")
        );
    }

    /// An unmatched name falls back to the built-in default palette (TermLab
    /// Dark), silently (no error, no panic) — the name-lookup `else` branch
    /// of `resolve_theme_in` when the value isn't a path and isn't found in
    /// either dir. This is the one fallback path the settings-picker's
    /// deleted `preview_theme_colors` stopgap used to exercise indirectly;
    /// Task 5's removal of that command (and its tests) left it with no
    /// direct coverage anywhere in the workspace until this test.
    /// Distinctive values, not just "some non-empty scheme," so a fallback
    /// to the wrong palette reds it.
    #[test]
    fn resolve_theme_falls_back_to_the_default_palette_for_an_unmatched_name() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();

        let scheme = resolve_theme_in(
            "definitely_not_a_real_theme_xyz",
            &[bundled.path().to_path_buf(), user.path().to_path_buf()],
        );
        assert_eq!(scheme.primary.background, "#282a36", "TermLab Dark bg");
        assert_eq!(scheme.primary.foreground, "#f8f8f2", "TermLab Dark fg");
        assert_eq!(scheme.normal.red, "#ff5555", "TermLab Dark red");
    }

    #[test]
    fn default_color_scheme_primary_colors() {
        let cs = ColorScheme::default();
        assert_eq!(cs.primary.background, "#282a36", "TermLab Dark background");
        assert_eq!(cs.primary.foreground, "#f8f8f2", "TermLab Dark foreground");
        assert_eq!(
            cs.primary.dim_foreground.as_deref(),
            Some("#6272a4"),
            "TermLab Dark dim foreground"
        );
        assert_eq!(
            cs.primary.bright_foreground.as_deref(),
            Some("#ffffff"),
            "TermLab Dark bright foreground"
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
    fn list_themes_finds_bundled_termlab_light_theme_exactly_once() {
        let themes = list_themes();
        let count = themes.keys().filter(|k| *k == "TermLab Light").count();
        assert_eq!(
            count,
            1,
            "expected 'TermLab Light' to be discoverable exactly once, found: {:?}",
            themes.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn list_themes_finds_bundled_termlab_dark_theme_exactly_once() {
        let themes = list_themes();
        let count = themes.keys().filter(|k| *k == "TermLab Dark").count();
        assert_eq!(
            count,
            1,
            "expected 'TermLab Dark' to be discoverable exactly once, found: {:?}",
            themes.keys().collect::<Vec<_>>()
        );
    }

    /// `frontend/themes/TermLab Dark.toml` carries the app's default terminal
    /// palette, so the file-backed resolution and the hardcoded
    /// `ColorScheme::default()` must agree field for field: the file is what
    /// the picker offers and what `auto` selects under a dark appearance,
    /// while the Default impl is what every unmatched/broken name falls back
    /// to. If they drift, the same nominal palette renders two different ways
    /// depending on which path a user reaches it by.
    ///
    /// The file is deliberately NOT the vendored upstream alacritty-theme
    /// `dracula.toml` this palette descends from (that stays a
    /// parse-fixture-only file at
    /// `termlab_core/tests/fixtures/alacritty-themes/dracula.toml`) — see the
    /// discriminator test below.
    ///
    /// This is the strongest possible pin: direct equality (no `PartialEq`
    /// derive on these structs, so compared field by field). Keep
    /// `crates/termlab_tauri/frontend/themes/TermLab Dark.toml` in sync with
    /// `ColorScheme::default()` below, or this test reds.
    #[test]
    fn resolve_theme_termlab_dark_is_byte_identical_to_the_hardcoded_default() {
        let file_backed = resolve_theme("TermLab Dark");
        let hardcoded = ColorScheme::default();

        assert_eq!(file_backed.primary.background, hardcoded.primary.background);
        assert_eq!(file_backed.primary.foreground, hardcoded.primary.foreground);
        assert_eq!(
            file_backed.primary.dim_foreground,
            hardcoded.primary.dim_foreground
        );
        assert_eq!(
            file_backed.primary.bright_foreground,
            hardcoded.primary.bright_foreground
        );

        assert_eq!(
            file_backed.normal.as_array(),
            hardcoded.normal.as_array(),
            "normal ANSI colors"
        );
        assert_eq!(
            file_backed.bright.as_array(),
            hardcoded.bright.as_array(),
            "bright ANSI colors"
        );

        assert!(
            file_backed.dim.is_none() && hardcoded.dim.is_none(),
            "neither source defines colors.dim"
        );

        let (file_cursor, default_cursor) = (
            file_backed.cursor.as_ref().expect("file defines cursor"),
            hardcoded.cursor.as_ref().expect("default defines cursor"),
        );
        assert_eq!(file_cursor.text, default_cursor.text);
        assert_eq!(file_cursor.cursor, default_cursor.cursor);

        let (file_selection, default_selection) = (
            file_backed
                .selection
                .as_ref()
                .expect("file defines selection"),
            hardcoded
                .selection
                .as_ref()
                .expect("default defines selection"),
        );
        assert_eq!(file_selection.text, default_selection.text);
        assert_eq!(file_selection.background, default_selection.background);

        assert!(
            file_backed.indexed_colors.is_empty() && hardcoded.indexed_colors.is_empty(),
            "neither source defines colors.indexed_colors"
        );
    }

    /// A regression alarm specifically for "someone swapped the vendored
    /// upstream alacritty-theme dracula fixture in as `frontend/themes/
    /// TermLab Dark.toml`" — the equality test above would catch that too
    /// (an upstream swap breaks equality with `ColorScheme::default()`), but
    /// this pins TermLab's actual hardcoded values directly, on the three
    /// ANSI fields where TermLab's longstanding built-in and upstream
    /// alacritty-theme's community fixture are known to disagree (see the
    /// vendored fixture's own `normal.black "#000000"`, `bright.black
    /// "#555555"`, `bright.red "#ff5555"` — all different from the values
    /// asserted below).
    #[test]
    fn resolve_theme_termlab_dark_pins_termlabs_hardcoded_colors_not_upstreams() {
        let scheme = resolve_theme("TermLab Dark");
        assert_eq!(
            scheme.normal.black, "#21222c",
            "TermLab's hardcoded value, not upstream alacritty-theme's #000000"
        );
        assert_eq!(
            scheme.bright.black, "#6272a4",
            "TermLab's hardcoded value, not upstream alacritty-theme's #555555"
        );
        assert_eq!(
            scheme.bright.red, "#ff6e6e",
            "TermLab's hardcoded value, not upstream alacritty-theme's #ff5555"
        );
    }

    #[test]
    fn resolve_theme_loads_termlab_light_by_exact_name() {
        let scheme = resolve_theme("TermLab Light");
        assert_eq!(scheme.primary.background, "#E3E8EF");
    }

    #[test]
    fn resolve_theme_termlab_light_full_snapshot() {
        let cs = resolve_theme("TermLab Light");

        assert_eq!(cs.primary.background, "#E3E8EF", "background");
        assert_eq!(cs.primary.foreground, "#1F2933", "foreground");

        let normal = cs.normal.as_array();
        assert_eq!(normal[0], "#1F2933", "normal black");
        assert_eq!(normal[1], "#B3261E", "normal red");
        assert_eq!(normal[2], "#1E7B34", "normal green");
        assert_eq!(normal[3], "#9A6700", "normal yellow");
        assert_eq!(normal[4], "#1D4ED8", "normal blue");
        assert_eq!(normal[5], "#8E24AA", "normal magenta");
        assert_eq!(normal[6], "#0E7490", "normal cyan");
        assert_eq!(normal[7], "#D5DBE3", "normal white");

        let bright = cs.bright.as_array();
        assert_eq!(bright[0], "#52606D", "bright black");
        assert_eq!(bright[1], "#D93025", "bright red");
        assert_eq!(bright[2], "#2E9E4C", "bright green");
        assert_eq!(bright[3], "#B8860B", "bright yellow");
        assert_eq!(bright[4], "#3B82F6", "bright blue");
        assert_eq!(bright[5], "#AB47BC", "bright magenta");
        assert_eq!(bright[6], "#0891B2", "bright cyan");
        assert_eq!(bright[7], "#F4F7FA", "bright white");

        let cursor = cs.cursor.expect("cursor colors present");
        assert_eq!(cursor.cursor, "#1F2933", "cursor color");
        assert_eq!(cursor.text, "#E3E8EF", "cursor text color");

        let selection = cs.selection.expect("selection colors present");
        assert_eq!(selection.background, "#CAD4E2", "selection background");
        assert_eq!(selection.text, "#1F2933", "selection foreground");
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

    /// Minimal required-fields TOML, plus one extra top-level `[colors.*]`
    /// table and one extra key inside `[colors.primary]` that don't
    /// correspond to any field on `ColorScheme`/`PrimaryColors`. Neither
    /// `AlacrittyThemeFile` nor its nested structs set
    /// `#[serde(deny_unknown_fields)]`, so `toml`/`serde` silently ignore
    /// unrecognized keys by default. This pins that behavior so a future
    /// accidental `deny_unknown_fields` addition fails loudly here instead
    /// of only when a real-world theme file breaks in production.
    #[test]
    fn unknown_keys_are_tolerated() {
        let toml_str = r##"
[colors.primary]
background = "#000000"
foreground = "#ffffff"
some_future_field_nobody_has_heard_of_yet = "#123456"

[colors.normal]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"

[colors.bright]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"

[colors.some_entirely_unknown_table]
whatever = "#000000"
"##;
        let theme: Result<AlacrittyThemeFile, _> = toml::from_str(toml_str);
        assert!(
            theme.is_ok(),
            "unknown keys must not fail parsing: {:?}",
            theme.err()
        );
    }

    /// Both color string forms Alacritty accepts must parse: `#rrggbb` and
    /// the legacy `0x`-prefixed form. Every color field on `ColorScheme` is
    /// a raw `String` (not a validated RGB type), so this is really testing
    /// that no validation was accidentally added that would reject `0x...`
    /// — see the module docs for why `0x` isn't exercised by a vendored
    /// fixture (no file in the upstream corpus uses it).
    #[test]
    fn deserialize_0x_prefixed_color_form() {
        let toml_str = r##"
[colors.primary]
background = "0x1e1e2e"
foreground = "0xcdd6f4"

[colors.normal]
black   = "0x45475a"
red     = "#f38ba8"
green   = "0xa6e3a1"
yellow  = "#f9e2af"
blue    = "0x89b4fa"
magenta = "#f5c2e7"
cyan    = "0x94e2d5"
white   = "#bac2de"

[colors.bright]
black   = "0x585b70"
red     = "#f38ba8"
green   = "0xa6e3a1"
yellow  = "#f9e2af"
blue    = "0x89b4fa"
magenta = "#f5c2e7"
cyan    = "0x94e2d5"
white   = "0xa6adc8"
"##;
        let theme: AlacrittyThemeFile = toml::from_str(toml_str).expect("0x colors must parse");
        assert_eq!(theme.colors.primary.background, "0x1e1e2e");
        assert_eq!(theme.colors.normal.black, "0x45475a");
        assert_eq!(theme.colors.bright.white, "0xa6adc8");
    }

    /// `cursor`/`vi_mode_cursor`/`selection`/`search`/`hints` all accept the
    /// `CellForeground`/`CellBackground` sentinel strings (which reference
    /// the affected cell's own colors) in place of a hex value. Since every
    /// field here is a raw `String`, these parse the same way any other
    /// string does — this test documents and pins that.
    #[test]
    fn deserialize_cell_rgb_sentinel_strings() {
        let toml_str = r##"
[colors.primary]
background = "#000000"
foreground = "#ffffff"

[colors.cursor]
text = "CellBackground"
cursor = "CellForeground"

[colors.selection]
text = "CellBackground"
background = "CellForeground"

[colors.search.matches]
foreground = "CellBackground"
background = "CellForeground"

[colors.normal]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"

[colors.bright]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"
"##;
        let theme: AlacrittyThemeFile =
            toml::from_str(toml_str).expect("CellRgb sentinel strings must parse");
        let cs = theme.colors;
        assert_eq!(cs.cursor.unwrap().text, "CellBackground");
        assert_eq!(cs.selection.unwrap().background, "CellForeground");
        assert_eq!(
            cs.search.unwrap().matches.unwrap().foreground,
            "CellBackground"
        );
    }

    /// `vi_mode_cursor`, `search`, `hints`, `line_indicator`, `footer_bar`,
    /// `indexed_colors`, `transparent_background_colors`, and
    /// `draw_bold_text_with_bright_colors` all parse into a typed
    /// representation, not just "ignored as an unknown key". This is the
    /// schema-completion contract: every field the current Alacritty schema
    /// defines is both ACCEPTED (parses without error, covered by the
    /// `unknown_keys_are_tolerated` test above for truly novel keys) and
    /// REPRESENTED (readable off `ColorScheme`, covered here).
    #[test]
    fn deserialize_full_extended_schema() {
        let toml_str = r##"
[colors]
transparent_background_colors = true
draw_bold_text_with_bright_colors = true

[colors.primary]
background = "#000000"
foreground = "#ffffff"

[colors.vi_mode_cursor]
text = "#111111"
cursor = "#222222"

[colors.search.matches]
foreground = "#333333"
background = "#444444"

[colors.search.focused_match]
foreground = "#555555"
background = "#666666"

[colors.hints.start]
foreground = "#777777"
background = "#888888"

[colors.hints.end]
foreground = "#999999"
background = "#aaaaaa"

[colors.line_indicator]
foreground = "#bbbbbb"
background = "#cccccc"

[colors.footer_bar]
foreground = "#dddddd"
background = "#eeeeee"

[[colors.indexed_colors]]
index = 16
color = "#ff0000"

[[colors.indexed_colors]]
index = 235
color = "#00ff00"

[colors.normal]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"

[colors.bright]
black = "#000000"
red = "#000000"
green = "#000000"
yellow = "#000000"
blue = "#000000"
magenta = "#000000"
cyan = "#000000"
white = "#000000"
"##;
        let theme: AlacrittyThemeFile =
            toml::from_str(toml_str).expect("full extended schema must parse");
        let cs = theme.colors;

        let vi_cursor = cs.vi_mode_cursor.expect("vi_mode_cursor present");
        assert_eq!(vi_cursor.text, "#111111");
        assert_eq!(vi_cursor.cursor, "#222222");

        let search = cs.search.expect("search present");
        assert_eq!(search.matches.unwrap().background, "#444444");
        assert_eq!(search.focused_match.unwrap().foreground, "#555555");

        let hints = cs.hints.expect("hints present");
        assert_eq!(hints.start.unwrap().background, "#888888");
        assert_eq!(hints.end.unwrap().foreground, "#999999");

        let line_indicator = cs.line_indicator.expect("line_indicator present");
        assert_eq!(line_indicator.foreground.as_deref(), Some("#bbbbbb"));

        let footer_bar = cs.footer_bar.expect("footer_bar present");
        assert_eq!(footer_bar.background.as_deref(), Some("#eeeeee"));

        assert_eq!(cs.indexed_colors.len(), 2);
        assert_eq!(cs.indexed_colors[0].index, 16);
        assert_eq!(cs.indexed_colors[0].color, "#ff0000");
        assert_eq!(cs.indexed_colors[1].index, 235);
        assert_eq!(cs.indexed_colors[1].color, "#00ff00");

        assert_eq!(cs.transparent_background_colors, Some(true));
        assert_eq!(cs.draw_bold_text_with_bright_colors, Some(true));
    }

    #[test]
    fn default_color_scheme_new_optional_fields_are_none_or_empty() {
        // Pins that extending the schema didn't change the built-in
        // fallback theme's applied behavior (zero-behavior-change for the
        // fields that already had meaning).
        let cs = ColorScheme::default();
        assert!(cs.vi_mode_cursor.is_none());
        assert!(cs.search.is_none());
        assert!(cs.hints.is_none());
        assert!(cs.line_indicator.is_none());
        assert!(cs.footer_bar.is_none());
        assert!(cs.indexed_colors.is_empty());
        assert!(cs.transparent_background_colors.is_none());
        assert!(cs.draw_bold_text_with_bright_colors.is_none());
    }

    // -----------------------------------------------------------------
    // list_theme_entries_in / PalettePreview
    // -----------------------------------------------------------------

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alacritty-themes")
    }

    #[test]
    fn entries_include_a_parsed_valid_theme_with_preview() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(bundled.path().join("Valid.toml"), theme_toml("#123456")).unwrap();

        let entries = list_theme_entries_in(bundled.path(), user.path());
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ThemeListEntry::Parsed {
                name,
                source,
                palette_preview,
                shadows_builtin,
            } => {
                assert_eq!(name, "Valid");
                assert_eq!(*source, ThemeSource::Builtin);
                assert_eq!(palette_preview.bg, "#123456");
                assert_eq!(palette_preview.fg, "#ffffff");
                assert!(!shadows_builtin);
            }
            ThemeListEntry::Broken { name, error } => {
                panic!(
                    "expected a parsed entry for 'Valid', got Broken {{ name: {name}, error: {error} }}"
                );
            }
        }
    }

    #[test]
    fn entries_include_a_broken_theme_as_an_error_entry() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(bundled.path().join("Broken.toml"), "not valid toml [[[").unwrap();

        let entries = list_theme_entries_in(bundled.path(), user.path());
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ThemeListEntry::Broken { name, error } => {
                assert_eq!(name, "Broken");
                assert!(
                    !error.is_empty(),
                    "expected a non-empty parse error message"
                );
            }
            ThemeListEntry::Parsed { name, .. } => {
                panic!("expected a Broken entry for '{name}', got Parsed")
            }
        }
    }

    #[test]
    fn entries_empty_when_both_dirs_are_empty() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();

        let entries = list_theme_entries_in(bundled.path(), user.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn entries_no_error_when_user_dir_is_missing_just_builtins() {
        let bundled = tempfile::tempdir().unwrap();
        std::fs::write(
            bundled.path().join("OnlyBuiltin.toml"),
            theme_toml("#000001"),
        )
        .unwrap();
        let missing_user_dir = bundled.path().join("does-not-exist");

        let entries = list_theme_entries_in(bundled.path(), &missing_user_dir);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ThemeListEntry::Parsed { name, source, .. } => {
                assert_eq!(name, "OnlyBuiltin");
                assert_eq!(*source, ThemeSource::Builtin);
            }
            ThemeListEntry::Broken { name, error } => {
                panic!(
                    "expected a parsed builtin entry, got Broken {{ name: {name}, error: {error} }}"
                );
            }
        }
    }

    #[test]
    fn entries_user_theme_shadows_builtin_of_the_same_name() {
        // Mirrors resolve_theme_prefers_user_dir_over_bundled_dir_on_name_collision,
        // but through the listing rather than resolution, using the real
        // production theme name so this exercises the exact scenario the
        // brief calls out ("user 'TermLab Dark.toml' -> one entry,
        // user-sourced, flagged").
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(
            bundled.path().join("TermLab Dark.toml"),
            theme_toml("#070A0E"),
        )
        .unwrap();
        std::fs::write(user.path().join("TermLab Dark.toml"), theme_toml("#111111")).unwrap();

        let entries = list_theme_entries_in(bundled.path(), user.path());
        assert_eq!(
            entries.len(),
            1,
            "the shadowed built-in must not appear as a separate entry"
        );
        match &entries[0] {
            ThemeListEntry::Parsed {
                name,
                source,
                palette_preview,
                shadows_builtin,
            } => {
                assert_eq!(name, "TermLab Dark");
                assert_eq!(*source, ThemeSource::User);
                assert_eq!(
                    palette_preview.bg, "#111111",
                    "the user theme's colors must win, not the shadowed built-in's"
                );
                assert!(*shadows_builtin);
            }
            ThemeListEntry::Broken { name, error } => {
                panic!(
                    "expected a parsed user entry, got Broken {{ name: {name}, error: {error} }}"
                );
            }
        }
    }

    #[test]
    fn entries_ordered_builtins_first_then_users_each_sorted_by_name() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        for name in ["Zeta", "Alpha"] {
            std::fs::write(
                bundled.path().join(format!("{name}.toml")),
                theme_toml("#000000"),
            )
            .unwrap();
        }
        for name in ["Yankee", "Bravo"] {
            std::fs::write(
                user.path().join(format!("{name}.toml")),
                theme_toml("#000000"),
            )
            .unwrap();
        }

        let entries = list_theme_entries_in(bundled.path(), user.path());
        let names: Vec<&str> = entries.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["Alpha", "Zeta", "Bravo", "Yankee"]);
    }

    #[test]
    fn palette_preview_from_known_fixture_matches_pinned_snapshot() {
        // Cross-checked against the pinned values in
        // tests/alacritty_fixtures.rs::dracula_snapshot.
        let scheme = load_theme(&fixtures_dir().join("dracula.toml")).expect("dracula.toml parses");
        let preview = PalettePreview::from_scheme(&scheme);

        assert_eq!(preview.bg, "#282a36");
        assert_eq!(preview.fg, "#f8f8f2");
        assert_eq!(
            preview.ansi,
            [
                "#000000", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd",
                "#bbbbbb", "#555555", "#ff5555", "#50fa7b", "#f1fa8c", "#caa9fa", "#ff79c6",
                "#8be9fd", "#ffffff",
            ]
        );
    }
}
