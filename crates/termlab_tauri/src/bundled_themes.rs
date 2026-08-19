//! Resolves the real on-disk location of the bundled theme TOMLs in a
//! packaged Tauri app, and injects it into `termlab_core::color_scheme` so
//! `bundled_themes_dir()` stops relying on its dev-only
//! `CARGO_MANIFEST_DIR`-relative fallback once the app is actually
//! installed (that fallback resolves inside a source checkout only — see
//! the `TODO(packaging)` history on `termlab_core::color_scheme::bundled_themes_dir`).
//!
//! ## Why this exists
//!
//! `crates/termlab_tauri/frontend/themes/*.toml` live under `frontend/`,
//! which `tauri.conf.json`'s `build.frontendDist` ("frontend") treats as
//! webview assets: they get sealed into the frontend asset archive that
//! ships inside the app binary, not left as loose files on disk. But
//! `termlab_core::color_scheme::bundled_themes_dir` reads its two built-ins
//! (`TermLab Dark.toml`, `TermLab Light.toml`, the palettes `auto` — the
//! default `colors.theme` — resolves to) with `std::fs::read_to_string`,
//! which cannot reach into that archive.
//!
//! So the same two files are ALSO listed under `tauri.conf.json`'s
//! `bundle.resources` map:
//!
//! ```json
//! "resources": { "frontend/themes/": "themes/" }
//! ```
//!
//! which tells the bundler to additionally copy them, as loose files, into
//! the packaged app's resource directory (`$RESOURCE/themes/*.toml`) — see
//! <https://v2.tauri.app/develop/resources/>. That directory is what this
//! module resolves via `app.path().resource_dir()`
//! (`tauri::path::PathResolver::resource_dir`, backed by
//! `tauri_utils::platform::resource_dir`) and injects with
//! `termlab_core::color_scheme::set_bundled_themes_dir`.
//!
//! ## Why injection is conditional, not unconditional
//!
//! In a dev run (`tauri dev` / `cargo run`), `bundle.resources` is never
//! copied anywhere — only `tauri build`/`tauri bundle` do that. `resource_dir()`
//! in dev typically resolves to the cargo output directory (e.g.
//! `target/debug`), which has no `themes/` subdirectory at all.
//! Unconditionally injecting that path would silently break `auto` in dev
//! (today it resolves fine via `bundled_themes_dir`'s `CARGO_MANIFEST_DIR`
//! fallback, which this module must not disturb). So injection only
//! happens when the resolved resource directory actually contains both
//! built-in TOMLs; otherwise this is a no-op and the dev fallback keeps
//! working exactly as before.
//!
//! ## What is and isn't verified here
//!
//! This module's own logic — "does this directory contain both built-in
//! TOMLs" — is unit tested directly (`themes_dir_has_builtins`). Resolving
//! an actual `resource_dir()` and injecting it requires a live
//! `tauri::AppHandle`, which needs a running app; that path is exercised
//! statically (this module compiles and the config parses — `cargo check -p
//! termlab_tauri`) but has NOT been verified against a real packaged
//! `.app`/`.dmg`, which would require actually running `tauri build`.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, Runtime};

/// The two files `termlab_core::effective_theme` needs the default `"auto"`
/// theme to resolve — see that module's docs for `TERMLAB_DARK_THEME` /
/// `TERMLAB_LIGHT_THEME`.
const REQUIRED_BUILTIN_THEMES: [&str; 2] = ["TermLab Dark.toml", "TermLab Light.toml"];

/// True when `dir` contains both required built-in theme files, i.e. it
/// looks like a real packaged resource dir rather than a dev-run
/// `resource_dir()` that happens to exist but was never seeded by
/// `tauri.conf.json`'s `bundle.resources`.
fn themes_dir_has_builtins(dir: &Path) -> bool {
    REQUIRED_BUILTIN_THEMES
        .iter()
        .all(|name| dir.join(name).is_file())
}

/// Resolve the packaged resource directory's `themes/` subdirectory, but
/// only when it looks real — see module docs for why a dev run intentionally
/// does not qualify.
fn resolve_packaged_themes_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let resource_dir = app.path().resource_dir().ok()?;
    let themes_dir = resource_dir.join("themes");
    themes_dir_has_builtins(&themes_dir).then_some(themes_dir)
}

/// Inject the packaged themes directory into `termlab_core::color_scheme`,
/// if one is found. Call once, as early as possible in
/// `tauri::Builder::setup`, before any command can resolve a theme (see
/// `crates/termlab_tauri/src/lib.rs`). A no-op — the dev fallback stays in
/// force — when running un-packaged or when the resolved resource dir
/// doesn't have the built-ins.
pub(crate) fn inject_bundled_themes_dir<R: Runtime>(app: &AppHandle<R>) {
    match resolve_packaged_themes_dir(app) {
        Some(dir) => {
            log::info!(
                "startup: found packaged bundled themes dir, injecting: {}",
                dir.display()
            );
            if !termlab_core::color_scheme::set_bundled_themes_dir(dir) {
                log::warn!(
                    "startup: bundled themes dir was already injected once; ignoring this call"
                );
            }
        }
        None => {
            log::debug!(
                "startup: no packaged bundled themes dir found (dev run, or resource dir \
                 missing the built-in themes) — termlab_core's dev CARGO_MANIFEST_DIR \
                 fallback remains in force"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_dir_has_builtins_true_when_both_files_present() {
        let dir = tempfile::tempdir().unwrap();
        for name in REQUIRED_BUILTIN_THEMES {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        assert!(themes_dir_has_builtins(dir.path()));
    }

    #[test]
    fn themes_dir_has_builtins_false_when_one_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TermLab Dark.toml"), "").unwrap();
        // "TermLab Light.toml" deliberately not written.
        assert!(!themes_dir_has_builtins(dir.path()));
    }

    #[test]
    fn themes_dir_has_builtins_false_when_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!themes_dir_has_builtins(dir.path()));
    }

    #[test]
    fn themes_dir_has_builtins_false_when_dir_does_not_exist() {
        // Models the dev-run case: resource_dir() resolves to something
        // real (e.g. target/debug), but its themes/ subdirectory was never
        // created because bundle.resources is only applied by `tauri build`.
        assert!(!themes_dir_has_builtins(Path::new(
            "/does/not/exist/at/all/ever"
        )));
    }
}
