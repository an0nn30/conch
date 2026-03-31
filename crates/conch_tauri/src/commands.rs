//! General Tauri commands: config queries, layout persistence, zoom, and
//! menu rebuilding.
//!
//! These are the "miscellaneous" commands that don't belong in a more specific
//! module like `pty` or `remote`.

use std::sync::Arc;

use conch_core::config;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::TauriState;
use crate::menu;
use crate::plugins;
use crate::theme;

// ---------------------------------------------------------------------------
// App config
// ---------------------------------------------------------------------------

/// Return general app config the frontend needs.
#[tauri::command]
pub(crate) fn get_app_config(state: tauri::State<'_, TauriState>) -> serde_json::Value {
    let cfg = state.config.read();
    let dec = format!("{:?}", cfg.window.decorations).to_lowercase();
    serde_json::json!({
        "appearance_mode": format!("{:?}", cfg.colors.appearance_mode).to_lowercase(),
        "zen_mode_shortcut": cfg.conch.keyboard.zen_mode,
        "decorations": dec,
        "platform": std::env::consts::OS,
        "debug_build": cfg!(debug_assertions),
        "notification_position": cfg.conch.ui.notification_position,
        "native_notifications": cfg.conch.ui.native_notifications,
        "ui_font_family": cfg.conch.ui.font_family,
        "ui_font_size": cfg.conch.ui.font_size,
        "ui_font_small": cfg.conch.ui.font.small,
        "ui_font_list": cfg.conch.ui.font.list,
        "ui_font_normal": cfg.conch.ui.font.normal,
    })
}

/// Return build/version info for the About dialog.
/// Build metadata is embedded at compile time by vergen-git2.
#[tauri::command]
pub(crate) fn get_about_info() -> serde_json::Value {
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("VERGEN_GIT_SHA").unwrap_or("dev"),
        "build_date": option_env!("VERGEN_GIT_COMMIT_TIMESTAMP").unwrap_or("unknown"),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })
}

#[tauri::command]
pub(crate) fn get_home_dir() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string())
}

#[tauri::command]
pub(crate) fn clipboard_read_text() -> Option<String> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    clipboard.get_text().ok()
}

#[tauri::command]
pub(crate) fn clipboard_write_text(text: String) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Theme colors
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn get_theme_colors(state: tauri::State<'_, TauriState>) -> theme::ThemeColors {
    let cfg = state.config.read();
    theme::resolve_theme_colors(&cfg)
}

// ---------------------------------------------------------------------------
// Terminal config (font, cursor, scroll)
// ---------------------------------------------------------------------------

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct TerminalDisplayConfig {
    font_family: String,
    font_size: f64,
    cursor_style: String,
    cursor_blink: bool,
    scroll_sensitivity: f64,
}

#[tauri::command]
pub(crate) fn get_terminal_config(state: tauri::State<'_, TauriState>) -> TerminalDisplayConfig {
    let cfg = state.config.read();
    let font = cfg.resolved_terminal_font();
    let cursor = &cfg.terminal.cursor.style;
    let cursor_style = match cursor.shape.to_lowercase().as_str() {
        "block" => "block",
        "underline" => "underline",
        "beam" | "bar" => "bar",
        _ => "block",
    }
    .to_string();

    TerminalDisplayConfig {
        font_family: font.normal.family.clone(),
        font_size: font.size as f64,
        cursor_style,
        cursor_blink: cursor.blinking,
        scroll_sensitivity: cfg.terminal.scroll_sensitivity as f64,
    }
}

// ---------------------------------------------------------------------------
// Keyboard config
// ---------------------------------------------------------------------------

/// Keyboard shortcuts exposed to the frontend.
#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct KeyboardShortcuts {
    toggle_right_panel: String,
    toggle_left_panel: String,
    toggle_bottom_panel: String,
    split_vertical: String,
    split_horizontal: String,
    close_pane: String,
    rename_tab: String,
}

#[tauri::command]
pub(crate) fn get_keyboard_shortcuts(state: tauri::State<'_, TauriState>) -> KeyboardShortcuts {
    let cfg = state.config.read();
    let kb = &cfg.conch.keyboard;
    KeyboardShortcuts {
        toggle_right_panel: kb.toggle_right_panel.clone(),
        toggle_left_panel: kb.toggle_left_panel.clone(),
        toggle_bottom_panel: kb.toggle_bottom_panel.clone(),
        split_vertical: kb.split_vertical.clone(),
        split_horizontal: kb.split_horizontal.clone(),
        close_pane: kb.close_pane.clone(),
        rename_tab: kb.rename_tab.clone(),
    }
}

// ---------------------------------------------------------------------------
// Window state persistence
// ---------------------------------------------------------------------------

/// Layout state sent from the frontend to persist.
#[derive(Deserialize)]
pub(crate) struct WindowLayout {
    ssh_panel_width: Option<f64>,
    ssh_panel_visible: Option<bool>,
    files_panel_width: Option<f64>,
    files_panel_visible: Option<bool>,
    bottom_panel_visible: Option<bool>,
}

/// Layout state sent to the frontend on load.
#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct SavedLayout {
    window_width: f64,
    window_height: f64,
    ssh_panel_width: f64,
    ssh_panel_visible: bool,
    files_panel_width: f64,
    files_panel_visible: bool,
    bottom_panel_visible: bool,
}

#[tauri::command]
pub(crate) fn app_ready(window: tauri::WebviewWindow) {
    let _ = window.show();
}

#[tauri::command]
pub(crate) fn open_devtools(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        window.open_devtools();
        return Ok(());
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = window;
        Err("Developer console is only available in debug builds.".to_string())
    }
}

#[tauri::command]
pub(crate) fn get_saved_layout() -> SavedLayout {
    let state = config::load_persistent_state().unwrap_or_default();
    SavedLayout {
        window_width: state.layout.window_width as f64,
        window_height: state.layout.window_height as f64,
        ssh_panel_width: state.layout.right_panel_width as f64,
        ssh_panel_visible: state.layout.right_panel_visible,
        files_panel_width: state.layout.left_panel_width as f64,
        files_panel_visible: state.layout.left_panel_visible,
        bottom_panel_visible: state.layout.bottom_panel_visible,
    }
}

#[tauri::command]
pub(crate) fn save_window_layout(window: tauri::WebviewWindow, layout: WindowLayout) {
    let size = window.inner_size().unwrap_or_default();
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical_w = size.width as f64 / scale;
    let logical_h = size.height as f64 / scale;

    let mut state = config::load_persistent_state().unwrap_or_default();
    state.layout.window_width = logical_w as f32;
    state.layout.window_height = logical_h as f32;
    if let Some(w) = layout.ssh_panel_width {
        state.layout.right_panel_width = w as f32;
    }
    if let Some(v) = layout.ssh_panel_visible {
        state.layout.right_panel_visible = v;
    }
    if let Some(w) = layout.files_panel_width {
        state.layout.left_panel_width = w as f32;
    }
    if let Some(v) = layout.files_panel_visible {
        state.layout.left_panel_visible = v;
    }
    if let Some(v) = layout.bottom_panel_visible {
        state.layout.bottom_panel_visible = v;
    }
    let _ = config::save_persistent_state(&state);
}

// ---------------------------------------------------------------------------
// Zoom
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn set_zoom_level(
    window: tauri::WebviewWindow,
    scale_factor: f64,
) -> Result<(), String> {
    window.set_zoom(scale_factor).map_err(|e| e.to_string())?;
    let mut state = config::load_persistent_state().unwrap_or_default();
    state.layout.zoom_factor = scale_factor as f32;
    let _ = config::save_persistent_state(&state);
    Ok(())
}

#[tauri::command]
pub(crate) fn get_zoom_level() -> f64 {
    let state = config::load_persistent_state().unwrap_or_default();
    let z = state.layout.zoom_factor as f64;
    if z > 0.0 { z } else { 1.0 }
}

// ---------------------------------------------------------------------------
// Window label
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn current_window_label(window: tauri::WebviewWindow) -> String {
    window.label().to_string()
}

#[tauri::command]
pub(crate) fn set_active_pane(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    pane_id: u32,
) {
    state
        .active_panes
        .lock()
        .insert(window.label().to_string(), pane_id);
}

// ---------------------------------------------------------------------------
// Menu rebuild
// ---------------------------------------------------------------------------

/// Rebuild the app menu including dynamically registered plugin menu items.
#[tauri::command]
pub(crate) fn rebuild_menu(
    app: tauri::AppHandle,
    plugin_state: tauri::State<'_, Arc<Mutex<plugins::PluginState>>>,
) -> Result<(), String> {
    let kb = config::load_user_config()
        .map(|c| c.conch.keyboard)
        .unwrap_or_default();

    let plugin_items = plugin_state.lock().menu_items.read().clone();

    // On Windows/Linux the custom titlebar handles menus; skip native menu.
    if cfg!(target_os = "windows") || cfg!(target_os = "linux") {
        return Ok(());
    }
    let new_menu = menu::build_app_menu_with_plugins(&app, &kb, &plugin_items)
        .map_err(|e| format!("Menu build failed: {e}"))?;
    app.set_menu(new_menu)
        .map_err(|e| format!("Set menu failed: {e}"))?;
    Ok(())
}
