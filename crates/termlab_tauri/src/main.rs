#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use termlab_core::config;

fn main() {
    // Platform init MUST run before anything else — fixes locale, PATH, and
    // SSH_AUTH_SOCK when launched from Finder/desktop (not a terminal).
    termlab_tauri::platform::init();

    env_logger::init();

    // Move a pre-rebrand ~/.config/conch directory to ~/.config/termlab
    // before anything reads config, state, vault, or themes.
    config::migrate_legacy_config_dir();

    let user_config = config::load_user_config().unwrap_or_else(|e| {
        log::error!("Failed to load config.toml, using defaults: {e:#}");
        config::UserConfig::default()
    });

    if let Err(e) = termlab_tauri::run(user_config) {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}
