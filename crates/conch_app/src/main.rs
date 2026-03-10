#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod dialogs;
mod extra_window;
mod icons;
mod input;
mod ipc;
mod plugins;
mod sessions;
mod shortcuts;
mod sidebar_handler;
mod ssh;
mod platform;
#[cfg(target_os = "macos")]
mod macos_menu;
mod mouse;
mod notifications;
mod state;
mod terminal;
mod ui;
mod watcher;

use std::sync::Arc;

use app::ConchApp;
use clap::{Parser, Subcommand};
use conch_core::config;

#[derive(Parser)]
#[command(name = "conch", about = "Conch terminal emulator")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Benchmark: log per-frame timing and resource usage to stderr.
    #[arg(long)]
    bench: bool,

    /// Benchmark with extra window (both visible).
    #[arg(long)]
    bench_extra: bool,

    /// Benchmark hidden-window mode: launch main + extra window, hide main,
    /// and log per-frame timing / resource usage to stderr.
    #[arg(long)]
    bench_hidden: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Send a message to a running Conch instance via IPC.
    Msg {
        #[command(subcommand)]
        action: MsgAction,
    },
    /// Check Lua plugin files for syntax errors and API misuse.
    Check {
        /// Plugin files to check.
        #[arg(required = true)]
        files: Vec<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum MsgAction {
    /// Create a new window.
    NewWindow {
        /// Working directory for the new window.
        #[arg(long, short = 'd')]
        working_directory: Option<String>,
    },
    /// Create a new tab in the focused window.
    NewTab {
        /// Working directory for the new tab.
        #[arg(long, short = 'd')]
        working_directory: Option<String>,
    },
}

fn load_app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../icons/app-icon.png"))
        .expect("Failed to decode app icon")
        .into_rgba8();
    let (w, h) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    }
}

/// Convert character-cell dimensions to pixel size for the initial window.
///
/// Uses rough estimates for cell size and UI chrome (tab bar, title bar padding).
/// The terminal will resize itself to fit once actual font metrics are measured.
fn window_size_from_config(cfg: &config::WindowDimensions) -> [f32; 2] {
    let cols = if cfg.columns == 0 { 150 } else { cfg.columns };
    let lines = if cfg.lines == 0 { 50 } else { cfg.lines };

    // Approximate cell size before font metrics are measured.
    let cell_w: f32 = 8.0;
    let cell_h: f32 = 16.0;
    // Extra chrome: tab bar (~30px), sidebar padding, window margins.
    let chrome_w: f32 = 40.0;
    let chrome_h: f32 = 50.0;

    [
        (cols as f32 * cell_w + chrome_w).max(600.0),
        (lines as f32 * cell_h + chrome_h).max(400.0),
    ]
}

/// Send an IPC message to a running Conch instance.
#[cfg(unix)]
fn send_ipc_message(msg: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let socket_path = ipc::ipc_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("Failed to connect to Conch at {}: {e}", socket_path.display()))?;
    stream
        .write_all(msg.as_bytes())
        .map_err(|e| format!("Failed to send message: {e}"))?;
    stream
        .write_all(b"\n")
        .map_err(|e| format!("Failed to send newline: {e}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn send_ipc_message(_msg: &str) -> Result<(), String> {
    Err("IPC is not supported on this platform".into())
}

fn main() -> eframe::Result<()> {
    // Perform platform-specific environment setup (locale, SSH_AUTH_SOCK, etc.)
    // before anything else.
    platform::init();

    let cli = Cli::parse();

    // Handle `conch check ...` — validate plugin files without launching the GUI.
    if let Some(Command::Check { files }) = &cli.command {
        let mut any_error = false;
        for path in files {
            let result = conch_plugin::check_plugin(path);
            let display_path = path.display();
            if result.diagnostics.is_empty() {
                println!("{display_path}: ok");
            } else {
                for diag in &result.diagnostics {
                    eprintln!("{display_path}:{diag}");
                }
            }
            if result.has_errors() {
                any_error = true;
            }
        }
        std::process::exit(if any_error { 1 } else { 0 });
    }

    // Handle `conch msg ...` subcommands — these don't launch the GUI.
    if let Some(Command::Msg { action }) = cli.command {
        let json = match action {
            MsgAction::NewWindow { working_directory } => {
                serde_json::json!({"type": "create_window", "working_directory": working_directory})
            }
            MsgAction::NewTab { working_directory } => {
                serde_json::json!({"type": "create_tab", "working_directory": working_directory})
            }
        };
        match send_ipc_message(&json.to_string()) {
            Ok(()) => {
                println!("Message sent.");
                return Ok(());
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }

    env_logger::init();

    // Load config early so we can size the window before creating the app.
    config::migrate_if_needed();
    let user_config = config::load_user_config().unwrap_or_else(|e| {
        log::error!("Failed to load config.toml, using defaults: {e:#}");
        config::UserConfig::default()
    });
    let persistent = config::load_persistent_state().unwrap_or_default();

    // Use persisted window size if available, otherwise fall back to config-based sizing.
    let window_size = if persistent.layout.window_width > 0.0 && persistent.layout.window_height > 0.0 {
        [persistent.layout.window_width, persistent.layout.window_height]
    } else {
        window_size_from_config(&user_config.window.dimensions)
    };

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime"),
    );

    let native_menu = cfg!(target_os = "macos") && user_config.conch.ui.native_menu_bar;

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(window_size)
        .with_icon(Arc::new(load_app_icon()));

    // Apply window decoration style from config.
    use config::WindowDecorations;
    match user_config.window.decorations {
        WindowDecorations::Full => {
            if cfg!(target_os = "macos") && !native_menu {
                // Transparent title bar: content extends behind it, we draw our own menu.
                viewport = viewport
                    .with_fullsize_content_view(true)
                    .with_titlebar_shown(true)
                    .with_title_shown(false);
            } else {
                viewport = viewport
                    .with_title_shown(true)
                    .with_titlebar_shown(true);
            }
        }
        WindowDecorations::Transparent => {
            viewport = viewport
                .with_fullsize_content_view(true)
                .with_titlebar_shown(true)
                .with_title_shown(false)
                .with_transparent(true);
        }
        WindowDecorations::Buttonless => {
            // No title bar at all — just the terminal content edge-to-edge.
            viewport = viewport
                .with_decorations(false)
                .with_transparent(true);
        }
        WindowDecorations::None => {
            viewport = viewport.with_decorations(false);
        }
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let bench = cli.bench;
    let bench_extra = cli.bench_extra;
    let bench_hidden = cli.bench_hidden;

    eframe::run_native(
        "Conch",
        options,
        Box::new(move |_cc| {
            let mut app = ConchApp::new(rt);
            if bench || bench_extra || bench_hidden {
                app.bench_hidden_mode = bench_hidden;
                app.bench_start = Some(std::time::Instant::now());
                app.bench_last_report = Some(std::time::Instant::now());
                if bench_extra {
                    // Spawn extra window but keep main visible.
                    app.bench_extra_mode = true;
                    eprintln!("[bench] Extra-window mode (both visible). Reporting every 2s.");
                } else if bench {
                    eprintln!("[bench] Normal mode. Reporting every 2s.");
                }
            }
            Ok(Box::new(app))
        }),
    )
}
