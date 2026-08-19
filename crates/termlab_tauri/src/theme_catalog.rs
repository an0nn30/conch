//! Terminal theme catalog for the settings picker.
//!
//! Layers a serializable, TS-exported shape on top of
//! `termlab_core::color_scheme`'s existing directory scan, parsing, and
//! user-shadows-built-in collision rule — this module adds no new theme
//! discovery or TOML parsing of its own. See
//! `termlab_core::color_scheme::list_theme_entries` for that machinery.

use serde::Serialize;
use ts_rs::TS;

use termlab_core::color_scheme::{
    PalettePreview as CorePalettePreview, ThemeListEntry as CoreThemeListEntry,
    ThemeSource as CoreThemeSource,
};

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PalettePreview {
    pub bg: String,
    pub fg: String,
    pub ansi: [String; 16],
}

impl From<CorePalettePreview> for PalettePreview {
    fn from(p: CorePalettePreview) -> Self {
        Self {
            bg: p.bg,
            fg: p.fg,
            ansi: p.ansi,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemeSource {
    Builtin,
    User,
}

impl From<CoreThemeSource> for ThemeSource {
    fn from(s: CoreThemeSource) -> Self {
        match s {
            CoreThemeSource::Builtin => ThemeSource::Builtin,
            CoreThemeSource::User => ThemeSource::User,
        }
    }
}

/// One entry returned by `list_terminal_themes`: either a theme that parsed
/// successfully (with its preview), or a broken theme file surfaced as a
/// `{ name, error }` pair so the picker can grey it out with the parse error
/// instead of hiding it. Untagged so the frontend sees exactly the two
/// documented shapes rather than an internal variant tag.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(untagged)]
pub(crate) enum ThemeListEntry {
    #[serde(rename_all = "camelCase")]
    Parsed {
        name: String,
        source: ThemeSource,
        // Boxed for the same reason as termlab_core::color_scheme::ThemeListEntry
        // (clippy::large_enum_variant) — see that type's doc comment.
        palette_preview: Box<PalettePreview>,
        shadows_builtin: bool,
    },
    Broken {
        name: String,
        error: String,
    },
}

impl From<CoreThemeListEntry> for ThemeListEntry {
    fn from(entry: CoreThemeListEntry) -> Self {
        match entry {
            CoreThemeListEntry::Parsed {
                name,
                source,
                palette_preview,
                shadows_builtin,
            } => ThemeListEntry::Parsed {
                name,
                source: source.into(),
                palette_preview: Box::new((*palette_preview).into()),
                shadows_builtin,
            },
            CoreThemeListEntry::Broken { name, error } => ThemeListEntry::Broken { name, error },
        }
    }
}

/// Pure conversion layer — no Tauri handles, so it is plain `cargo test`able
/// without spinning up a Tauri app.
pub(crate) fn build_theme_list() -> Vec<ThemeListEntry> {
    termlab_core::color_scheme::list_theme_entries()
        .into_iter()
        .map(ThemeListEntry::from)
        .collect()
}

#[tauri::command]
pub(crate) fn list_terminal_themes() -> Vec<ThemeListEntry> {
    build_theme_list()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_convert_from_core_entries() {
        let core_entries = vec![
            CoreThemeListEntry::Parsed {
                name: "Example".into(),
                source: CoreThemeSource::Builtin,
                palette_preview: Box::new(CorePalettePreview {
                    bg: "#000000".into(),
                    fg: "#ffffff".into(),
                    ansi: std::array::from_fn(|i| format!("#{i:02x}")),
                }),
                shadows_builtin: false,
            },
            CoreThemeListEntry::Broken {
                name: "Broken".into(),
                error: "boom".into(),
            },
        ];

        let converted: Vec<ThemeListEntry> =
            core_entries.into_iter().map(ThemeListEntry::from).collect();

        match &converted[0] {
            ThemeListEntry::Parsed {
                name,
                source,
                palette_preview,
                shadows_builtin,
            } => {
                assert_eq!(name, "Example");
                assert_eq!(*source, ThemeSource::Builtin);
                assert_eq!(palette_preview.bg, "#000000");
                assert!(!shadows_builtin);
            }
            ThemeListEntry::Broken { .. } => panic!("expected Parsed"),
        }

        match &converted[1] {
            ThemeListEntry::Broken { name, error } => {
                assert_eq!(name, "Broken");
                assert_eq!(error, "boom");
            }
            ThemeListEntry::Parsed { .. } => panic!("expected Broken"),
        }
    }

    #[test]
    fn build_theme_list_includes_production_builtins() {
        // Production directories: bundled frontend themes/ + user's
        // ~/.config/termlab/themes/ (which may or may not exist on the
        // machine running this test). Not hermetic by design — this pins
        // that the real bundled TermLab Dark/Light themes surface through
        // the full command-facing conversion, complementing the hermetic
        // tests in termlab_core::color_scheme.
        let entries = build_theme_list();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| match e {
                ThemeListEntry::Parsed { name, .. } => name.as_str(),
                ThemeListEntry::Broken { name, .. } => name.as_str(),
            })
            .collect();
        assert!(
            names.contains(&"TermLab Dark"),
            "expected 'TermLab Dark' in the theme list, found: {names:?}"
        );
        assert!(
            names.contains(&"TermLab Light"),
            "expected 'TermLab Light' in the theme list, found: {names:?}"
        );
    }

    /// TermLab ships exactly TWO built-in palettes — the pair `auto` picks
    /// between. Both must enumerate through the SAME command-facing path the
    /// settings picker consumes (`list_terminal_themes` -> `build_theme_list`)
    /// as real `Parsed` entries with `source: Builtin`, and nothing else may
    /// enumerate as a built-in (a third bundled file would show up as an
    /// unexplained extra row in the picker).
    #[test]
    fn build_theme_list_offers_exactly_two_builtins() {
        let entries = build_theme_list();
        let mut builtins: Vec<&str> = Vec::new();
        for entry in &entries {
            match entry {
                ThemeListEntry::Parsed { name, source, .. } => {
                    if *source == ThemeSource::Builtin {
                        builtins.push(name.as_str());
                    }
                }
                ThemeListEntry::Broken { name, error } => {
                    // A bundled file that stopped parsing would silently drop
                    // out of the count below; fail loudly instead.
                    assert!(
                        name != "TermLab Dark" && name != "TermLab Light",
                        "bundled '{name}' failed to parse: {error}"
                    );
                }
            }
        }
        // A user theme of the same name shadows the built-in (later-dirs-win),
        // in which case that entry is `source: User` and legitimately absent
        // from this list — so this is an upper bound plus a subset check, not
        // an exact set the developer's own ~/.config could perturb. The
        // "both names are present under SOME source" half is
        // build_theme_list_includes_production_builtins above.
        assert!(
            builtins.len() <= 2,
            "only TermLab Dark/Light may ship as built-ins, found: {builtins:?}"
        );
        for name in &builtins {
            assert!(
                *name == "TermLab Dark" || *name == "TermLab Light",
                "unexpected bundled built-in theme: {name}"
            );
        }
    }
}
