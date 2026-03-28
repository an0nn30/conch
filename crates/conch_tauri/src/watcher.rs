//! File watcher for theme hot-reload.
//!
//! Watches the themes/ directory for changes. On change, emits a
//! `config-changed` event so the frontend can re-fetch theme colors.
//! Config.toml changes are handled by the Settings dialog (save_settings).

use std::path::PathBuf;
use std::time::Duration;

use tauri::Emitter;

/// Start watching theme files. Returns a thread join handle, or `None` if the
/// thread could not be spawned.
pub fn start(app_handle: tauri::AppHandle) -> Option<std::thread::JoinHandle<()>> {
    match std::thread::Builder::new()
        .name("theme-watcher".into())
        .spawn(move || {
            watch_loop(app_handle);
        }) {
        Ok(handle) => Some(handle),
        Err(e) => {
            log::error!("Failed to spawn theme watcher thread: {e}");
            None
        }
    }
}

fn watch_loop(app: tauri::AppHandle) {
    use std::collections::HashMap;
    use std::fs;

    let themes_dir = conch_core::color_scheme::themes_dir();

    let mut mtimes: HashMap<PathBuf, std::time::SystemTime> = HashMap::new();

    seed_dir_mtimes(&themes_dir, &mut mtimes);

    loop {
        std::thread::sleep(Duration::from_secs(2));

        let mut changed = false;

        // Check themes directory for modified .toml files.
        if themes_dir.exists() {
            if let Ok(entries) = fs::read_dir(&themes_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "toml") {
                        if let Ok(meta) = fs::metadata(&path) {
                            if let Ok(mtime) = meta.modified() {
                                if mtimes.get(&path) != Some(&mtime) {
                                    mtimes.insert(path, mtime);
                                    changed = true;
                                    log::info!("Theme file changed, reloading");
                                }
                            }
                        }
                    }
                }
            }
        }

        if changed {
            let _ = app.emit("config-changed", ());
        }
    }
}

fn seed_dir_mtimes(
    dir: &PathBuf,
    mtimes: &mut std::collections::HashMap<PathBuf, std::time::SystemTime>,
) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    mtimes.insert(path, mtime);
                }
            }
        }
    }
}
