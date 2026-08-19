//! Alacritty-compatible color theme loading.
//!
//! Deserializes unmodified Alacritty `.toml` theme files (e.g. dracula.toml,
//! catppuccin_mocha.toml) and provides a built-in Dracula fallback.
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
//! Only a subset of the accepted schema currently has terminal meaning and
//! is APPLIED further down the pipeline (`termlab_tauri::theme`): `primary`,
//! `cursor`, `selection`, `normal`, `bright`. `dim` and `indexed_colors` are
//! parsed and preserved on [`ColorScheme`] but are not yet threaded through
//! `ThemeColors`/xterm — see the crate-level task-1 report for the carry
//! gap. `vi_mode_cursor`, `search`, `hints`, `line_indicator`, `footer_bar`,
//! `transparent_background_colors`, and `draw_bold_text_with_bright_colors`
//! are parsed and otherwise ignored by design (no terminal-chrome meaning
//! here today).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Return the bundled themes directory shipped with the frontend:
/// `crates/termlab_tauri/frontend/themes/`, resolved at compile time relative
/// to this crate's manifest dir (siblings under `crates/`).
// TODO(packaging): env!("CARGO_MANIFEST_DIR") only resolves inside a source
// checkout — it is dev-only and will not point at the right location in a
// packaged/release build. Before shipping any bundled or release build,
// replace this with Tauri's resource resolver (bundle themes as Tauri
// resources) or read from an installed config/resource directory, or
// bundled themes will silently vanish at runtime.
fn bundled_themes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("termlab_tauri")
        .join("frontend")
        .join("themes")
}

/// Return the user themes directory: `~/.config/termlab/themes/`.
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

/// Resolve a theme by name or path: load from disk or fall back to built-in Dracula.
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
                    "Failed to load theme from '{}': {e}, using built-in Dracula",
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
    fn list_themes_finds_bundled_termlab_light_theme_exactly_once() {
        let themes = list_themes();
        let count = themes.keys().filter(|k| *k == "TermLab Light").count();
        assert_eq!(
            count, 1,
            "expected 'TermLab Light' to be discoverable exactly once, found: {:?}",
            themes.keys().collect::<Vec<_>>()
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
}
