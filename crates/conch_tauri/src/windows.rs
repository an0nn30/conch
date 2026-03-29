//! Window creation and management.
//!
//! Handles creating new Conch windows with proper sizing, decorations, theme,
//! and zoom level from persisted state.

use std::sync::atomic::{AtomicU32, Ordering};

use conch_core::config;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

// ---------------------------------------------------------------------------
// Window ID counter
// ---------------------------------------------------------------------------

pub(crate) static NEXT_WINDOW_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Appearance helper
// ---------------------------------------------------------------------------

/// Convert the user's appearance mode to a Tauri window theme.
pub(crate) fn appearance_to_theme(
    mode: &conch_core::config::AppearanceMode,
) -> Option<tauri::Theme> {
    match mode {
        conch_core::config::AppearanceMode::Dark => Some(tauri::Theme::Dark),
        conch_core::config::AppearanceMode::Light => Some(tauri::Theme::Light),
        conch_core::config::AppearanceMode::System => None,
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Tauri command to open a new window (used by custom titlebar menu).
///
/// Window creation must happen on the main thread.  Tauri commands run on a
/// thread-pool, so we dispatch via `run_on_main_thread` to avoid a deadlock
/// (the builder's `build()` posts to the main thread and waits, but the main
/// thread may be blocked waiting for this command to finish).
#[tauri::command]
pub(crate) async fn open_new_window(app: tauri::AppHandle) -> Result<(), String> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if let Err(e) = create_new_window(&handle) {
            log::error!("Failed to create new window: {e}");
        }
    })
    .map_err(|e| e.to_string())
}

/// Tauri command to open the settings in a dedicated window.
///
/// If a settings window is already open, focuses it instead of creating a
/// duplicate.
#[tauri::command]
pub(crate) async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let handle = app.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        // Focus existing settings window if already open.
        if let Some(win) = handle.get_webview_window("settings") {
            let _ = win.set_focus();
            let _ = tx.send(Ok(()));
            return;
        }
        let result = create_settings_window(&handle).map_err(|e| {
            log::error!("Failed to create settings window: {e}");
            e.to_string()
        });
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

fn create_settings_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    let user_cfg = config::load_user_config().unwrap_or_default();
    let theme = appearance_to_theme(&user_cfg.colors.appearance_mode);

    // Use custom titlebar on Windows/Linux (same as main window).
    let use_custom_titlebar = cfg!(target_os = "windows") || cfg!(target_os = "linux");

    let win = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Settings — Conch")
        .inner_size(780.0, 560.0)
        .resizable(true)
        .min_inner_size(600.0, 400.0)
        .decorations(!use_custom_titlebar)
        .theme(theme)
        .build()?;

    // Remove the app-level menu from this window so it has no menu bar.
    let _ = win.remove_menu();

    Ok(())
}

pub(crate) fn create_new_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    let label = loop {
        let id = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = format!("window-{id}");
        if app.get_webview_window(&candidate).is_none() {
            break candidate;
        }
    };

    let persisted = config::load_persistent_state().unwrap_or_default();
    let w = if persisted.layout.window_width > 100.0 {
        persisted.layout.window_width as f64
    } else {
        1200.0
    };
    let h = if persisted.layout.window_height > 100.0 {
        persisted.layout.window_height as f64
    } else {
        800.0
    };

    let user_cfg = config::load_user_config().unwrap_or_default();
    let user_wants_dec = !matches!(
        user_cfg.window.decorations,
        conch_core::config::WindowDecorations::None
            | conch_core::config::WindowDecorations::Buttonless
    );
    let use_custom_titlebar = cfg!(target_os = "windows") || cfg!(target_os = "linux");
    let dec = if use_custom_titlebar {
        false
    } else {
        user_wants_dec
    };
    let theme = appearance_to_theme(&user_cfg.colors.appearance_mode);

    let new_win = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("Conch")
        .inner_size(w, h)
        .resizable(true)
        .decorations(dec)
        .theme(theme)
        .visible(false)
        .build()?;
    let zoom = persisted.layout.zoom_factor;
    if zoom > 0.0 && (zoom - 1.0).abs() > f32::EPSILON {
        let _ = new_win.set_zoom(zoom as f64);
    }
    Ok(())
}
