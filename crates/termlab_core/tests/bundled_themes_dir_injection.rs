//! End-to-end coverage for `color_scheme::set_bundled_themes_dir`: the seam
//! `termlab_tauri`'s `setup()` uses to inject the real bundled-themes
//! directory in a packaged build (see
//! `crates/termlab_tauri/src/bundled_themes.rs`).
//!
//! This lives in its own integration test binary — not in
//! `color_scheme.rs`'s `#[cfg(test)] mod tests` — deliberately: calling the
//! real `set_bundled_themes_dir` poisons the process-global override for
//! the rest of whatever binary calls it (that's the whole point of a
//! once-only injection seam). Isolating it in its own `tests/*.rs` file
//! means that poisoning only affects this process, not the crate's regular
//! unit tests or `effective_theme.rs`'s tests, which rely on the unset
//! dev-fallback behavior.

use std::fs;
use std::path::PathBuf;

use termlab_core::color_scheme::{resolve_theme, set_bundled_themes_dir};
use termlab_core::config::ColorsConfig;
use termlab_core::effective_theme::{AUTO_THEME_NAME, resolve_effective_theme};

/// The real frontend themes directory: `crates/termlab_tauri/frontend/themes/`.
fn real_bundled_themes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("termlab_tauri")
        .join("frontend")
        .join("themes")
}

/// Build a temp dir standing in for a packaged app's resolved resource
/// directory, seeded with copies of the two real built-in TOMLs — mirroring
/// exactly what `bundled_themes::inject_bundled_themes_dir` finds under
/// `resource_dir().join("themes")` once `tauri.conf.json`'s
/// `bundle.resources` has copied them there.
fn packaged_themes_dir_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    for name in ["TermLab Dark.toml", "TermLab Light.toml"] {
        let src = real_bundled_themes_dir().join(name);
        let contents = fs::read_to_string(&src)
            .unwrap_or_else(|e| panic!("read real bundled theme {}: {e}", src.display()));
        fs::write(dir.path().join(name), contents).expect("write fixture copy");
    }
    dir
}

fn colors_config(theme: &str) -> ColorsConfig {
    ColorsConfig {
        theme: theme.into(),
        ..ColorsConfig::default()
    }
}

/// The full contract in one test, in this exact order, because
/// `set_bundled_themes_dir` can only be meaningfully exercised once per
/// process:
///
/// 1. before any injection, theme resolution still uses the dev fallback
///    (proves the override starts unset in a fresh process);
/// 2. the first `set_bundled_themes_dir` call succeeds and immediately wins
///    over that fallback for every subsequent resolution, including through
///    `effective_theme::resolve_effective_theme`'s `"auto"` path — the exact
///    pipeline `termlab_tauri` drives at runtime;
/// 3. a second call is a no-op: it returns `false` and does not disturb the
///    already-injected directory.
#[test]
fn injected_dir_wins_over_dev_fallback_and_injection_is_once_only() {
    // 1. Sanity: before injecting anything, "auto" resolves through the dev
    // fallback and finds the real bundled TermLab Dark/Light palettes.
    let dark_before = resolve_effective_theme(&colors_config(AUTO_THEME_NAME), "dark");
    let light_before = resolve_effective_theme(&colors_config(AUTO_THEME_NAME), "light");
    assert_ne!(
        dark_before.primary.background, light_before.primary.background,
        "dev fallback must resolve auto to two distinct built-in palettes"
    );

    // 2. Inject a temp dir standing in for a packaged app's resource dir.
    let packaged = packaged_themes_dir_fixture();
    let first_call = set_bundled_themes_dir(packaged.path().to_path_buf());
    assert!(first_call, "first injection call must take effect");

    // Prove the injected dir is what actually gets read now, not just that
    // resolution still succeeds: overwrite the copy on disk after
    // injecting, with a value distinguishable from both the real bundled
    // theme and the built-in Dracula fallback, and confirm resolve_theme
    // reads that exact byte content back out.
    let marker_background = "#00ff00";
    fs::write(
        packaged.path().join("TermLab Dark.toml"),
        format!(
            r##"
[colors.primary]
background = "{marker_background}"
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
        ),
    )
    .expect("overwrite injected TermLab Dark.toml");

    let dark_after = resolve_theme("TermLab Dark");
    assert_eq!(
        dark_after.primary.background, marker_background,
        "resolve_theme must read the injected directory, not the dev fallback"
    );

    // And the full "auto" pipeline, through effective_theme, must also see
    // the injected directory end to end.
    let auto_dark = resolve_effective_theme(&colors_config(AUTO_THEME_NAME), "dark");
    let auto_light = resolve_effective_theme(&colors_config(AUTO_THEME_NAME), "light");
    assert_eq!(auto_dark.primary.background, marker_background);
    assert_ne!(
        auto_dark.primary.background, auto_light.primary.background,
        "auto must still track appearance through the injected directory"
    );

    // 3. A second injection call must be a no-op: it reports failure and
    // must not replace the already-injected directory.
    let other_dir = tempfile::tempdir().expect("create second temp dir");
    let second_call = set_bundled_themes_dir(other_dir.path().to_path_buf());
    assert!(!second_call, "a second injection call must be rejected");

    let dark_still_after = resolve_theme("TermLab Dark");
    assert_eq!(
        dark_still_after.primary.background, marker_background,
        "the second injection attempt must not have taken effect"
    );
}
