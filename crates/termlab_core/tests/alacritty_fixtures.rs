//! The executable half of the Alacritty compatibility contract.
//!
//! Every file under `tests/fixtures/alacritty-themes/` is a REAL Alacritty
//! theme vendored verbatim from github.com/alacritty/alacritty-theme (see
//! `SOURCE.txt` and `LICENSE-alacritty-theme.txt` alongside the fixtures).
//! Each one must parse without error, and its resolved palette is pinned
//! with exact-value assertions so that a future change to `color_scheme.rs`
//! that silently reinterprets a color is caught here. Adding a fixture that
//! fails to parse is the intended regression alarm (spec: "Compatibility
//! contract").
//!
//! `tests/fixtures/broken/not_a_valid_theme.toml` is NOT vendored — it is a
//! hand-written malformed file used only to assert that a broken theme
//! fails to parse (`Result::Err`, not a panic).

use std::path::{Path, PathBuf};

use termlab_core::color_scheme::load_theme;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alacritty-themes")
}

fn broken_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/broken/not_a_valid_theme.toml")
}

/// Coarse safety net: every `.toml` file actually present under
/// `alacritty-themes/` parses. This is what "adding a fixture that fails to
/// parse is the regression alarm" means in practice — drop a new file in
/// the directory and this test exercises it with no further wiring.
#[test]
fn every_vendored_fixture_parses() {
    let dir = fixtures_dir();
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures dir must exist") {
        let entry = entry.expect("readable dir entry");
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            checked += 1;
            load_theme(&path)
                .unwrap_or_else(|e| panic!("fixture {} failed to parse: {e}", path.display()));
        }
    }
    assert!(
        checked >= 12,
        "expected at least 12 vendored fixtures, found {checked}"
    );
}

#[test]
fn dracula_snapshot() {
    let cs = load_theme(&fixtures_dir().join("dracula.toml")).expect("dracula.toml parses");
    assert_eq!(cs.primary.background, "#282a36");
    assert_eq!(cs.primary.foreground, "#f8f8f2");
    assert_eq!(cs.primary.dim_foreground, None);
    assert_eq!(cs.primary.bright_foreground, None);
    assert_eq!(
        cs.normal.as_array(),
        [
            "#000000", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#bbbbbb"
        ]
    );
    assert_eq!(
        cs.bright.as_array(),
        [
            "#555555", "#ff5555", "#50fa7b", "#f1fa8c", "#caa9fa", "#ff79c6", "#8be9fd", "#ffffff"
        ]
    );
    assert!(cs.dim.is_none());
    assert!(cs.cursor.is_none());
    assert!(cs.selection.is_none());
    assert!(cs.indexed_colors.is_empty());
}

#[test]
fn monokai_snapshot_minimal_shape() {
    // primary/normal/bright only: no cursor, selection, dim, or any
    // extended section — the floor of a valid theme file.
    let cs = load_theme(&fixtures_dir().join("monokai.toml")).expect("monokai.toml parses");
    assert_eq!(cs.primary.background, "#272822");
    assert_eq!(cs.primary.foreground, "#f8f8f2");
    assert_eq!(
        cs.normal.as_array(),
        [
            "#272822", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff", "#a1efe4", "#f8f8f2"
        ]
    );
    assert_eq!(
        cs.bright.as_array(),
        [
            "#75715e", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff", "#a1efe4", "#f9f8f5"
        ]
    );
    assert!(cs.dim.is_none());
    assert!(cs.cursor.is_none());
    assert!(cs.vi_mode_cursor.is_none());
    assert!(cs.selection.is_none());
    assert!(cs.search.is_none());
    assert!(cs.hints.is_none());
    assert!(cs.indexed_colors.is_empty());
}

#[test]
fn alacritty_0_12_snapshot_dim_hints_search_no_cursor_section() {
    let cs = load_theme(&fixtures_dir().join("alacritty_0_12.toml"))
        .expect("alacritty_0_12.toml parses");
    assert_eq!(cs.primary.background, "#1d1f21");
    assert_eq!(cs.primary.foreground, "#c5c8c6");

    // No [colors.cursor] / [colors.selection] table in this file at all —
    // confirms the optional sections stay None rather than erroring or
    // defaulting to something else when wholly absent.
    assert!(cs.cursor.is_none());
    assert!(cs.selection.is_none());

    let dim = cs.dim.as_ref().expect("dim present");
    assert_eq!(
        dim.as_array(),
        [
            "#131415", "#864343", "#777c44", "#9e824c", "#556a7d", "#75617b", "#5b7d78", "#828482"
        ]
    );

    let hints = cs.hints.as_ref().expect("hints present");
    let start = hints.start.as_ref().expect("hints.start present");
    assert_eq!(start.foreground, "#1d1f21");
    assert_eq!(start.background, "#e9ff5e");
    let end = hints.end.as_ref().expect("hints.end present");
    assert_eq!(end.foreground, "#e9ff5e");
    assert_eq!(end.background, "#1d1f21");

    let search = cs.search.as_ref().expect("search present");
    let matches = search.matches.as_ref().expect("search.matches present");
    assert_eq!(matches.foreground, "#000000");
    assert_eq!(matches.background, "#ffffff");
    let focused = search
        .focused_match
        .as_ref()
        .expect("search.focused_match present");
    assert_eq!(focused.foreground, "#ffffff");
    assert_eq!(focused.background, "#000000");
}

#[test]
fn konsole_linux_snapshot_dim_bright_foreground_cell_rgb_sentinels() {
    let cs = load_theme(&fixtures_dir().join("konsole_linux.toml"))
        .expect("konsole_linux.toml parses");
    assert_eq!(cs.primary.background, "#1f1f1f");
    assert_eq!(cs.primary.foreground, "#e3e3e3");
    assert_eq!(cs.primary.bright_foreground.as_deref(), Some("#ffffff"));
    assert_eq!(cs.primary.dim_foreground, None);

    let cursor = cs.cursor.as_ref().expect("cursor present");
    assert_eq!(cursor.text, "#191622");
    assert_eq!(cursor.cursor, "#f8f8f2");

    let dim = cs.dim.as_ref().expect("dim present");
    assert_eq!(dim.black, "#000000");
    assert_eq!(dim.white, "#b2b2b2");

    // search.matches uses ordinary hex; search.focused_match uses the
    // CellForeground/CellBackground sentinel strings instead of hex.
    let search = cs.search.as_ref().expect("search present");
    let matches = search.matches.as_ref().expect("search.matches present");
    assert_eq!(matches.foreground, "#b2b2b2");
    assert_eq!(matches.background, "#b26818");
    let focused = search
        .focused_match
        .as_ref()
        .expect("search.focused_match present");
    assert_eq!(focused.foreground, "CellBackground");
    assert_eq!(focused.background, "CellForeground");

    assert!(cs.selection.is_none());
    assert!(cs.vi_mode_cursor.is_none());
}

#[test]
fn github_dark_snapshot_indexed_colors() {
    let cs =
        load_theme(&fixtures_dir().join("github_dark.toml")).expect("github_dark.toml parses");
    assert_eq!(cs.primary.background, "#24292e");
    assert_eq!(cs.primary.foreground, "#d1d5da");
    assert_eq!(cs.indexed_colors.len(), 2);
    assert_eq!(cs.indexed_colors[0].index, 16);
    assert_eq!(cs.indexed_colors[0].color, "#d18616");
    assert_eq!(cs.indexed_colors[1].index, 17);
    assert_eq!(cs.indexed_colors[1].color, "#f97583");
    assert!(cs.dim.is_none());
    assert!(cs.cursor.is_none());
}

#[test]
fn catppuccin_mocha_snapshot_kitchen_sink() {
    // The richest fixture: dim_foreground, bright_foreground,
    // vi_mode_cursor, search, footer_bar, hints, selection, dim — every
    // optional section except indexed_colors, line_indicator, and the
    // two top-level booleans.
    let cs = load_theme(&fixtures_dir().join("catppuccin_mocha.toml"))
        .expect("catppuccin_mocha.toml parses");
    assert_eq!(cs.primary.background, "#1E1E2E");
    assert_eq!(cs.primary.foreground, "#CDD6F4");
    assert_eq!(cs.primary.dim_foreground.as_deref(), Some("#CDD6F4"));
    assert_eq!(cs.primary.bright_foreground.as_deref(), Some("#CDD6F4"));

    let cursor = cs.cursor.as_ref().expect("cursor present");
    assert_eq!(cursor.text, "#1E1E2E");
    assert_eq!(cursor.cursor, "#F5E0DC");

    let vi_cursor = cs.vi_mode_cursor.as_ref().expect("vi_mode_cursor present");
    assert_eq!(vi_cursor.text, "#1E1E2E");
    assert_eq!(vi_cursor.cursor, "#B4BEFE");

    let selection = cs.selection.as_ref().expect("selection present");
    assert_eq!(selection.text, "#1E1E2E");
    assert_eq!(selection.background, "#F5E0DC");

    let footer_bar = cs.footer_bar.as_ref().expect("footer_bar present");
    assert_eq!(footer_bar.foreground.as_deref(), Some("#1E1E2E"));
    assert_eq!(footer_bar.background.as_deref(), Some("#A6ADC8"));

    let search = cs.search.as_ref().expect("search present");
    assert_eq!(
        search.matches.as_ref().unwrap().background,
        "#A6ADC8"
    );
    assert_eq!(
        search.focused_match.as_ref().unwrap().background,
        "#A6E3A1"
    );

    let hints = cs.hints.as_ref().expect("hints present");
    assert_eq!(hints.start.as_ref().unwrap().background, "#F9E2AF");
    assert_eq!(hints.end.as_ref().unwrap().background, "#A6ADC8");

    let dim = cs.dim.as_ref().expect("dim present");
    assert_eq!(dim.black, "#45475A");
    assert_eq!(dim.white, "#BAC2DE");

    assert!(cs.line_indicator.is_none());
    assert!(cs.indexed_colors.is_empty());
}

#[test]
fn catppuccin_latte_snapshot_light_kitchen_sink() {
    let cs = load_theme(&fixtures_dir().join("catppuccin_latte.toml"))
        .expect("catppuccin_latte.toml parses");
    assert_eq!(cs.primary.background, "#EFF1F5");
    assert_eq!(cs.primary.foreground, "#4C4F69");
    let vi_cursor = cs.vi_mode_cursor.as_ref().expect("vi_mode_cursor present");
    assert_eq!(vi_cursor.cursor, "#7287FD");
    let dim = cs.dim.as_ref().expect("dim present");
    assert_eq!(dim.black, "#5C5F77");
}

#[test]
fn rose_pine_snapshot_vi_mode_cursor_and_hints_shape() {
    let cs = load_theme(&fixtures_dir().join("rose_pine.toml")).expect("rose_pine.toml parses");
    assert_eq!(cs.primary.background, "#191724");
    assert_eq!(cs.primary.foreground, "#e0def4");

    let cursor = cs.cursor.as_ref().expect("cursor present");
    assert_eq!(cursor.cursor, "#524f67");
    let vi_cursor = cs.vi_mode_cursor.as_ref().expect("vi_mode_cursor present");
    assert_eq!(vi_cursor.cursor, "#524f67");

    let selection = cs.selection.as_ref().expect("selection present");
    assert_eq!(selection.background, "#403d52");

    let hints = cs.hints.as_ref().expect("hints present");
    assert_eq!(hints.start.as_ref().unwrap().foreground, "#908caa");
    assert_eq!(hints.end.as_ref().unwrap().foreground, "#6e6a86");

    assert!(cs.dim.is_none());
    assert!(cs.search.is_none());
}

#[test]
fn gruvbox_dark_snapshot() {
    let cs =
        load_theme(&fixtures_dir().join("gruvbox_dark.toml")).expect("gruvbox_dark.toml parses");
    assert_eq!(cs.primary.background, "#282828");
    assert_eq!(cs.primary.foreground, "#ebdbb2");
    assert_eq!(cs.normal.black, "#282828");
    assert_eq!(cs.bright.white, "#ebdbb2");
    assert!(cs.dim.is_none());
    assert!(cs.cursor.is_none());
}

#[test]
fn nord_snapshot() {
    let cs = load_theme(&fixtures_dir().join("nord.toml")).expect("nord.toml parses");
    assert_eq!(cs.primary.background, "#2E3440");
    assert_eq!(cs.primary.foreground, "#D8DEE9");
    assert_eq!(cs.normal.blue, "#81A1C1");
    assert_eq!(cs.bright.cyan, "#8FBCBB");
}

#[test]
fn solarized_light_snapshot() {
    let cs = load_theme(&fixtures_dir().join("solarized_light.toml"))
        .expect("solarized_light.toml parses");
    assert_eq!(cs.primary.background, "#fdf6e3");
    assert_eq!(cs.primary.foreground, "#586e75");
    assert_eq!(cs.normal.yellow, "#b58900");
    assert_eq!(cs.bright.white, "#fdf6e3");
}

#[test]
fn one_dark_snapshot() {
    let cs = load_theme(&fixtures_dir().join("one_dark.toml")).expect("one_dark.toml parses");
    assert_eq!(cs.primary.background, "#282c34");
    assert_eq!(cs.primary.foreground, "#abb2bf");
    assert_eq!(cs.normal.red, "#e06c75");
    assert_eq!(cs.bright.white, "#ffffff");
}

/// A broken theme file must error, not panic — the "deliberately-broken
/// fixture" half of the contract. This is a hand-written synthetic file
/// (see `tests/fixtures/broken/not_a_valid_theme.toml`), not vendored.
#[test]
fn broken_fixture_errors_without_panicking() {
    let result = load_theme(&broken_fixture_path());
    assert!(
        result.is_err(),
        "a syntactically/structurally broken theme file must return Err, not Ok"
    );
}
