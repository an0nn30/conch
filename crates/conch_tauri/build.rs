fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Embed git commit hash and timestamp for the About dialog.
    // Uses vergen-git2 for cross-platform support (works on macOS, Linux, Windows).
    // Sets VERGEN_GIT_SHA, VERGEN_GIT_COMMIT_TIMESTAMP, etc.
    let git = vergen_git2::Git2Builder::all_git()?;
    vergen_git2::Emitter::default()
        .add_instructions(&git)?
        .emit()?;

    tauri_build::build();
    Ok(())
}
