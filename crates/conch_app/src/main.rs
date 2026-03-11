#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod extra_window;
mod input;
mod ipc;
mod mouse;
mod platform;
mod sessions;
mod shortcuts;
mod state;
mod tab_bar;
mod terminal;
mod watcher;

use std::sync::Arc;

use app::ConchApp;
use clap::{Parser, Subcommand};
use conch_core::config;

/// Apply the configured appearance mode to egui and the native window chrome.
pub(crate) fn apply_appearance_mode(ctx: &egui::Context, mode: config::AppearanceMode) {
    let (theme_pref, sys_theme) = match mode {
        config::AppearanceMode::Dark => (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
        config::AppearanceMode::Light => (egui::ThemePreference::Light, egui::SystemTheme::Light),
        config::AppearanceMode::System => (egui::ThemePreference::System, egui::SystemTheme::SystemDefault),
    };
    ctx.set_theme(theme_pref);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(sys_theme));
}

#[derive(Parser)]
#[command(name = "conch", about = "Conch terminal emulator")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Send a message to a running Conch instance via IPC.
    Msg {
        #[command(subcommand)]
        action: MsgAction,
    },
}

#[derive(Subcommand)]
enum MsgAction {
    /// Create a new window.
    NewWindow {
        #[arg(long, short = 'd')]
        working_directory: Option<String>,
    },
    /// Create a new tab in the focused window.
    NewTab {
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
fn window_size_from_config(cfg: &config::WindowDimensions) -> [f32; 2] {
    let cols = if cfg.columns == 0 { 150 } else { cfg.columns };
    let lines = if cfg.lines == 0 { 50 } else { cfg.lines };

    let cell_w: f32 = 8.0;
    let cell_h: f32 = 16.0;
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
    platform::init();

    let cli = Cli::parse();

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

    let user_config = config::load_user_config().unwrap_or_else(|e| {
        log::error!("Failed to load config.toml, using defaults: {e:#}");
        config::UserConfig::default()
    });
    let persistent = config::load_persistent_state().unwrap_or_default();

    // Use persisted window size if available.
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

    use config::WindowDecorations;
    match user_config.window.decorations {
        WindowDecorations::Full => {
            if cfg!(target_os = "macos") && !native_menu {
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

    let appearance_mode = user_config.colors.appearance_mode;

    eframe::run_native(
        "Conch",
        options,
        Box::new(move |cc| {
            apply_appearance_mode(&cc.egui_ctx, appearance_mode);
            Ok(Box::new(ConchApp::new(rt)))
        }),
    )
}
