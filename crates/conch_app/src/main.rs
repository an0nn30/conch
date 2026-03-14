#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod context_menu;
mod extra_window;
mod host;
mod icons;
mod input;
mod ipc;
mod menu_bar;
mod mouse;
mod platform;
mod sessions;
mod shortcuts;
mod state;
mod tab_bar;
mod terminal;
mod ui_theme;
mod watcher;

use std::sync::Arc;

use app::ConchApp;
use clap::{Parser, Subcommand};
use conch_core::config;

/// Load the platform's preferred UI font and set it as egui's default proportional font.
fn setup_system_ui_font(ctx: &egui::Context) {
    #[cfg(target_os = "linux")]
    {
        log::info!("Using egui built-in proportional font (Linux)");
        return;
    }

    #[cfg(target_os = "macos")]
    let font_data = load_macos_system_font();

    #[cfg(target_os = "windows")]
    let font_data = load_system_font_by_name(&["Segoe UI Variable", "Segoe UI"]);

    #[cfg(not(target_os = "linux"))]
    match font_data {
        Some((name, data)) => {
            log::info!("Font data loaded: {} bytes from '{name}'", data.len());
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "system-ui".to_owned(),
                egui::FontData::from_owned(data).into(),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "system-ui".to_owned());
            ctx.set_fonts(fonts);
            log::info!("UI font set to '{name}'");
        }
        None => {
            log::warn!("Could not load system UI font, using egui default");
        }
    }
}

/// On macOS, load SF Pro (San Francisco) by scanning the system font directory.
#[cfg(target_os = "macos")]
fn load_macos_system_font() -> Option<(String, Vec<u8>)> {
    // SF Pro is stored in /System/Library/Fonts/ but isn't queryable by
    // normal family name via Core Text. Read the font file directly.
    let sf_paths = [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSText.ttf",
        "/System/Library/Fonts/SF-Pro.ttf",
        "/System/Library/Fonts/SF-Pro-Text-Regular.otf",
        "/Library/Fonts/SF-Pro.ttf",
        "/Library/Fonts/SF-Pro-Text-Regular.otf",
    ];

    for path in &sf_paths {
        log::info!("Trying SF font path: {path}");
        if let Ok(data) = std::fs::read(path) {
            log::info!("Loaded system font from {path} ({} bytes)", data.len());
            return Some(("San Francisco".to_string(), data));
        }
    }

    // Fallback: use font-kit to find SF Pro or Helvetica Neue
    log::info!("SF font files not found at known paths, trying font-kit fallback");
    load_system_font_by_name(&["SF Pro", "SF Pro Text", "Helvetica Neue"])
}

/// Use font-kit to load a font by trying a list of family names in order.
#[cfg(not(target_os = "linux"))]
fn load_system_font_by_name(names: &[&str]) -> Option<(String, Vec<u8>)> {
    use font_kit::family_name::FamilyName;
    use font_kit::properties::Properties;
    use font_kit::source::SystemSource;

    let source = SystemSource::new();
    for name in names {
        log::info!("Trying system font: '{name}'");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            source.select_best_match(
                &[FamilyName::Title(name.to_string())],
                &Properties::new(),
            )
        }));
        let handle = match result {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                log::info!("Font '{name}' not found: {e}");
                continue;
            }
            Err(_) => {
                log::warn!("Font '{name}' caused a panic in font-kit, skipping");
                continue;
            }
        };

        let load_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle.load()));
        match load_result {
            Ok(Ok(font)) => {
                let full_name = font.full_name();
                log::info!("Matched font: full_name='{full_name}' (from query '{name}')");
                if let Some(data) = font.copy_font_data() {
                    return Some((full_name, (*data).to_vec()));
                }
                log::warn!("Could not copy font data for '{full_name}'");
            }
            Ok(Err(e)) => log::warn!("Could not load font '{name}': {e}"),
            Err(_) => log::warn!("Font '{name}' load panicked in font-kit, skipping"),
        }
    }
    None
}

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

/// Apply window decoration settings to a viewport builder using platform capabilities.
pub(crate) fn build_viewport(
    mut builder: egui::ViewportBuilder,
    decorations: config::WindowDecorations,
    platform: &platform::PlatformCapabilities,
) -> egui::ViewportBuilder {
    use config::WindowDecorations;
    match decorations {
        WindowDecorations::Full => {
            if platform.fullsize_content_view {
                builder = builder
                    .with_fullsize_content_view(true)
                    .with_titlebar_shown(true)
                    .with_title_shown(false);
            } else {
                builder = builder
                    .with_title_shown(true)
                    .with_titlebar_shown(true);
            }
        }
        WindowDecorations::Transparent => {
            builder = builder
                .with_fullsize_content_view(true)
                .with_titlebar_shown(true)
                .with_title_shown(false)
                .with_transparent(true);
        }
        WindowDecorations::Buttonless => {
            builder = builder
                .with_decorations(false)
                .with_transparent(true);
        }
        WindowDecorations::None => {
            builder = builder.with_decorations(false);
        }
    }
    builder
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

    let platform = platform::PlatformCapabilities::current();
    let decorations = platform.effective_decorations(user_config.window.decorations);

    let base_viewport = egui::ViewportBuilder::default()
        .with_inner_size(window_size)
        .with_icon(Arc::new(load_app_icon()));

    let viewport = build_viewport(base_viewport, decorations, &platform);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let appearance_mode = user_config.colors.appearance_mode;

    eframe::run_native(
        "Conch",
        options,
        Box::new(move |cc| {
            setup_system_ui_font(&cc.egui_ctx);
            apply_appearance_mode(&cc.egui_ctx, appearance_mode);
            Ok(Box::new(ConchApp::new(rt)))
        }),
    )
}
