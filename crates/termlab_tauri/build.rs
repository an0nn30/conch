fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Embed git commit hash and timestamp for the About dialog.
    // Uses vergen-git2 for cross-platform support (works on macOS, Linux, Windows).
    // Sets VERGEN_GIT_SHA, VERGEN_GIT_COMMIT_TIMESTAMP, etc.
    let git = vergen_git2::Git2Builder::all_git()?;
    vergen_git2::Emitter::default()
        .add_instructions(&git)?
        .emit()?;

    ensure_vendor_bundle();
    require_common_controls_v6_in_tests();

    tauri_build::build();
    Ok(())
}

/// Give Windows test binaries the same comctl32 v6 manifest the app gets.
///
/// `TaskDialogIndirect` is exported only by comctl32 version 6, which lives in
/// WinSxS and is bound only for executables whose manifest declares a
/// dependency on Microsoft.Windows.Common-Controls 6.0.0.0. `tauri_build`
/// embeds that manifest into the app binary, but `cargo test` links a separate
/// executable that never received it — so the loader bound
/// System32\comctl32.dll (v5.82), found no `TaskDialogIndirect`, and killed the
/// process at load with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) before `main`.
///
/// The whole `termlab_tauri` lib test target was unrunnable on Windows because
/// of it, on both x86_64 and aarch64.
fn require_common_controls_v6_in_tests() {
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }
    // `-tests` scopes this to test targets; the app binary is already covered
    // by the manifest tauri_build embeds.
    println!(
        "cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' \
         name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
         processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
}

/// Keep the generated CodeMirror bundle in step with its sources on every
/// cargo build, so a plain `cargo build`/`cargo run` can never ship a stale
/// or missing bundle again.
///
/// Why this exists: the bundle is git-ignored output of the frontend's npm
/// build step, which only Tauri's beforeBuildCommand/beforeDevCommand runs —
/// plain cargo never did. That has now bitten three times, most recently vim
/// mode silently doing nothing because a checkout carried a bundle predating
/// the vim export. Staleness is judged by mtimes against the inputs below;
/// when nothing changed this emits only rerun-if-changed lines and exits.
///
/// Machines without npm (CI's test jobs) get a loud warning instead of a
/// failure: the Rust tests don't need the bundle, and every release path has
/// its own explicit vendor step. A machine WITH npm where the build fails is
/// a real error and fails the build.
fn ensure_vendor_bundle() {
    use std::path::Path;
    use std::process::Command;

    let frontend = Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend");
    let bundle = frontend
        .join("vendor")
        .join("codemirror")
        .join("codemirror.js");
    let inputs = [
        "vendor-entry.mjs",
        "package-lock.json",
        "build-vendor.mjs",
        "check-vendor.mjs",
    ];
    for f in inputs {
        println!("cargo:rerun-if-changed={}", frontend.join(f).display());
    }
    // The bundle itself too: deleting it must trigger a rerun.
    println!("cargo:rerun-if-changed={}", bundle.display());

    let bundle_mtime = std::fs::metadata(&bundle).and_then(|m| m.modified()).ok();
    let newest_input = inputs
        .iter()
        .filter_map(|f| {
            std::fs::metadata(frontend.join(f))
                .and_then(|m| m.modified())
                .ok()
        })
        .max();
    let stale = match (bundle_mtime, newest_input) {
        (None, _) => true,
        (Some(b), Some(i)) => i > b,
        (Some(_), None) => false,
    };
    if !stale {
        return;
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    if Command::new(npm).arg("--version").output().is_err() {
        println!(
            "cargo:warning=frontend vendor bundle is stale or missing and npm is not installed; \
             the editor will be degraded. Install Node and rebuild, or run \
             `npm --prefix crates/termlab_tauri/frontend ci && npm --prefix crates/termlab_tauri/frontend run build:vendor`."
        );
        return;
    }

    if !frontend.join("node_modules").exists() {
        let status = Command::new(npm)
            .arg("ci")
            .current_dir(&frontend)
            .status()
            .expect("failed to spawn npm ci");
        assert!(status.success(), "npm ci failed in {}", frontend.display());
    }

    // build:vendor also runs check-vendor.mjs, so a mistyped export fails the
    // cargo build here rather than shipping a language that never highlights.
    let status = Command::new(npm)
        .args(["run", "build:vendor"])
        .current_dir(&frontend)
        .status()
        .expect("failed to spawn npm run build:vendor");
    assert!(
        status.success(),
        "npm run build:vendor failed — the frontend vendor bundle could not be built"
    );
}
