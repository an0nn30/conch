#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use termlab_core::config;

fn main() {
    // Logging comes first so every later startup step is diagnosable. Anything
    // that runs before this logs into the void — which is exactly what makes a
    // startup hang show up as no window and an empty log.
    env_logger::init();
    log::info!("startup: termlab {} starting", env!("CARGO_PKG_VERSION"));

    // Dual-mode CLI dispatch: `termlab <path...>` either forwards the paths
    // to an already-running instance over the IPC socket and exits, or (if
    // nothing is listening) falls through to a normal app boot carrying the
    // paths as pending opens. Must run before any Tauri/platform setup so a
    // forwarding invocation never pays for a second app instance.
    let pending_paths = match termlab_tauri::cli::run_cli_if_requested() {
        termlab_tauri::cli::CliAction::Exit(code) => std::process::exit(code),
        termlab_tauri::cli::CliAction::RunApp { pending_paths } => pending_paths,
    };

    // Platform init fixes locale, PATH, and SSH_AUTH_SOCK when launched from
    // Finder/desktop (not a terminal). It must run before any child process is
    // spawned, and it spawns environment probes of its own.
    termlab_tauri::platform::init();
    log::info!("startup: platform init complete");

    // Move a pre-rebrand ~/.config/conch directory to ~/.config/termlab
    // before anything reads config, state, vault, or themes.
    config::migrate_legacy_config_dir();

    let user_config = config::load_user_config().unwrap_or_else(|e| {
        log::error!("Failed to load config.toml, using defaults: {e:#}");
        config::UserConfig::default()
    });
    log::info!("startup: config loaded");

    if let Err(e) = termlab_tauri::run(user_config, pending_paths) {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}
