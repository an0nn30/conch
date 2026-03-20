//! Tauri-based UI for Conch (experimental).
//!
//! Uses xterm.js in a webview for terminal rendering, with a raw PTY backend
//! via `portable-pty`. This bypasses alacritty_terminal entirely — xterm.js
//! handles all terminal emulation.

mod ipc;
pub mod platform;
mod pty_backend;
pub(crate) mod plugins;
pub(crate) mod remote;
pub(crate) mod settings;
pub(crate) mod theme;
pub(crate) mod utf8_stream;
mod watcher;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use conch_core::config::{self, UserConfig};
use parking_lot::Mutex;
use pty_backend::PtyBackend;
use remote::RemoteState;
use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// Convert the user's appearance mode to a Tauri window theme.
fn appearance_to_theme(mode: &conch_core::config::AppearanceMode) -> Option<tauri::Theme> {
    match mode {
        conch_core::config::AppearanceMode::Dark => Some(tauri::Theme::Dark),
        conch_core::config::AppearanceMode::Light => Some(tauri::Theme::Light),
        conch_core::config::AppearanceMode::System => None,
    }
}

const MENU_NEW_TAB_ID: &str = "file.new_tab";
const MENU_CLOSE_TAB_ID: &str = "file.close_tab";
const MENU_NEW_WINDOW_ID: &str = "file.new_window";
const MENU_TOGGLE_LEFT_PANEL_ID: &str = "view.toggle_left_panel";
const MENU_TOGGLE_RIGHT_PANEL_ID: &str = "view.toggle_right_panel";
const MENU_FOCUS_SESSIONS_ID: &str = "view.focus_sessions";
const MENU_ZEN_MODE_ID: &str = "view.zen_mode";
const MENU_ZOOM_IN_ID: &str = "view.zoom_in";
const MENU_ZOOM_OUT_ID: &str = "view.zoom_out";
const MENU_ZOOM_RESET_ID: &str = "view.zoom_reset";
const MENU_PLUGIN_MANAGER_ID: &str = "tools.plugin_manager";
const MENU_MANAGE_TUNNELS_ID: &str = "tools.manage_tunnels";
const MENU_ACTION_EVENT: &str = "menu-action";
const MENU_ACTION_NEW_TAB: &str = "new-tab";
const MENU_ACTION_CLOSE_TAB: &str = "close-tab";
const MENU_ACTION_TOGGLE_LEFT_PANEL: &str = "toggle-left-panel";
const MENU_ACTION_TOGGLE_RIGHT_PANEL: &str = "toggle-right-panel";
const MENU_ACTION_FOCUS_SESSIONS: &str = "focus-sessions";
const MENU_ACTION_ZEN_MODE: &str = "zen-mode";
const MENU_ACTION_ZOOM_IN: &str = "zoom-in";
const MENU_ACTION_ZOOM_OUT: &str = "zoom-out";
const MENU_ACTION_ZOOM_RESET: &str = "zoom-reset";
const MENU_ACTION_PLUGIN_MANAGER: &str = "plugin-manager";
const MENU_ACTION_MANAGE_TUNNELS: &str = "manage-tunnels";
const MENU_SSH_EXPORT_ID: &str = "file.ssh_export";
const MENU_SSH_IMPORT_ID: &str = "file.ssh_import";
const MENU_ACTION_SSH_EXPORT: &str = "ssh-export";
const MENU_ACTION_SSH_IMPORT: &str = "ssh-import";

static NEXT_WINDOW_ID: AtomicU32 = AtomicU32::new(1);

pub(crate) struct TauriState {
    ptys: Arc<Mutex<HashMap<String, PtyBackend>>>,
    config: Mutex<UserConfig>,
}

#[derive(Clone, serde::Serialize)]
struct PtyOutputEvent {
    window_label: String,
    tab_id: u32,
    data: String,
}

#[derive(Clone, serde::Serialize)]
struct PtyExitEvent {
    window_label: String,
    tab_id: u32,
}

#[derive(Clone, serde::Serialize)]
struct MenuActionEvent {
    window_label: String,
    action: String,
}

fn resolved_shell(shell: &conch_core::config::TerminalShell) -> (Option<&str>, &[String]) {
    let program = shell.program.trim();
    if program.is_empty() {
        (None, &[])
    } else {
        (Some(program), shell.args.as_slice())
    }
}

fn session_key(window_label: &str, tab_id: u32) -> String {
    format!("{window_label}:{tab_id}")
}

#[tauri::command]
fn spawn_shell(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    tab_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    let key = session_key(&window_label, tab_id);
    let cfg = state.config.lock();
    let (shell, shell_args) = resolved_shell(&cfg.terminal.shell);

    let backend = PtyBackend::new(cols, rows, shell, shell_args, &cfg.terminal.env)
        .map_err(|e| format!("Failed to spawn PTY: {e}"))?;
    drop(cfg);

    let reader = backend
        .try_clone_reader()
        .ok_or("Failed to clone PTY reader")?;

    {
        let mut ptys = state.ptys.lock();
        if ptys.contains_key(&key) {
            return Err(format!(
                "Tab {tab_id} already exists on window {window_label}"
            ));
        }
        ptys.insert(key.clone(), backend);
    }

    let ptys = Arc::clone(&state.ptys);
    std::thread::Builder::new()
        .name(format!("pty-reader-{window_label}-{tab_id}"))
        .spawn(move || {
            pty_reader_loop(&app, &ptys, key, window_label, tab_id, reader);
        })
        .map_err(|e| format!("Failed to spawn PTY reader thread: {e}"))?;

    Ok(())
}

#[tauri::command]
fn write_to_pty(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    tab_id: u32,
    data: String,
) -> Result<(), String> {
    let key = session_key(window.label(), tab_id);
    let guard = state.ptys.lock();
    let pty = guard.get(&key).ok_or("PTY not spawned")?;
    pty.write(data.as_bytes()).map_err(|e| format!("{e}"))
}

#[tauri::command]
fn resize_pty(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, TauriState>,
    tab_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let key = session_key(window.label(), tab_id);
    let guard = state.ptys.lock();
    let pty = guard.get(&key).ok_or("PTY not spawned")?;
    pty.resize(cols, rows).map_err(|e| format!("{e}"))
}

#[tauri::command]
fn close_pty(window: tauri::WebviewWindow, state: tauri::State<'_, TauriState>, tab_id: u32) {
    let key = session_key(window.label(), tab_id);
    state.ptys.lock().remove(&key);
}

#[tauri::command]
fn current_window_label(window: tauri::WebviewWindow) -> String {
    window.label().to_string()
}

/// Rebuild the app menu including dynamically registered plugin menu items.
#[tauri::command]
fn rebuild_menu(
    app: tauri::AppHandle,
    plugin_state: tauri::State<'_, Arc<Mutex<plugins::PluginState>>>,
) -> Result<(), String> {
    let kb = config::load_user_config()
        .map(|c| c.conch.keyboard)
        .unwrap_or_default();

    let plugin_items = plugin_state.lock().menu_items.lock().clone();

    // On Windows the custom titlebar handles menus; skip native menu.
    if cfg!(target_os = "windows") {
        return Ok(());
    }
    let menu = build_app_menu_with_plugins(&app, &kb, &plugin_items)
        .map_err(|e| format!("Menu build failed: {e}"))?;
    app.set_menu(menu)
        .map_err(|e| format!("Set menu failed: {e}"))?;
    Ok(())
}

fn build_app_menu_with_plugins<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    keyboard: &conch_core::config::KeyboardConfig,
    plugin_items: &[plugins::PluginMenuItem],
) -> tauri::Result<Menu<R>> {
    // Build the base menu.
    let base = build_app_menu(app, keyboard)?;

    // If there are plugin menu items, rebuild the Tools menu to include them.
    if !plugin_items.is_empty() {
        // We can't easily modify an existing menu, so rebuild it fully.
        // For now, the plugin items are added to the Tools menu via
        // the on_menu_event handler. The menu IDs use "plugin.{plugin}.{action}".
        let mut tools_items: Vec<Box<dyn tauri::menu::IsMenuItem<R>>> = Vec::new();

        let plugin_manager = MenuItem::with_id(
            app,
            MENU_PLUGIN_MANAGER_ID,
            "Plugin Manager\u{2026}",
            true,
            None::<&str>,
        )?;
        tools_items.push(Box::new(plugin_manager));
        tools_items.push(Box::new(PredefinedMenuItem::separator(app)?));

        let manage_tunnels = MenuItem::with_id(
            app,
            MENU_MANAGE_TUNNELS_ID,
            "Manage SSH Tunnels\u{2026}",
            true,
            Some("CmdOrCtrl+Shift+T"),
        )?;
        tools_items.push(Box::new(manage_tunnels));

        // Add plugin items.
        if !plugin_items.is_empty() {
            tools_items.push(Box::new(PredefinedMenuItem::separator(app)?));
        }
        for item in plugin_items {
            let menu_id = format!("plugin.{}.{}", item.plugin, item.action);
            let accel = item.keybind.as_deref().map(|k| config_key_to_accelerator(k));
            let mi = MenuItem::with_id(
                app,
                &menu_id,
                &item.label,
                true,
                accel.as_deref(),
            )?;
            tools_items.push(Box::new(mi));
        }

        // Rebuild the tools submenu.
        let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = tools_items.iter().map(|b| &**b).collect();
        let new_tools = Submenu::with_items(app, "Tools", true, &refs)?;

        // Rebuild full menu bar with new tools menu.
        let new_tab = MenuItem::with_id(app, MENU_NEW_TAB_ID, "New Tab", true, Some("CmdOrCtrl+T"))?;
        let close_tab = MenuItem::with_id(app, MENU_CLOSE_TAB_ID, "Close Tab", true, Some("CmdOrCtrl+W"))?;
        let new_window = MenuItem::with_id(app, MENU_NEW_WINDOW_ID, "New Window", true, Some("CmdOrCtrl+Shift+N"))?;
        let separator = PredefinedMenuItem::separator(app)?;
        let close_window = PredefinedMenuItem::close_window(app, None)?;
        let ssh_export = MenuItem::with_id(app, MENU_SSH_EXPORT_ID, "Export", true, None::<&str>)?;
        let ssh_import = MenuItem::with_id(app, MENU_SSH_IMPORT_ID, "Import", true, None::<&str>)?;
        let ssh_manager_menu = Submenu::with_items(app, "SSH Manager", true, &[&ssh_export, &ssh_import])?;
        let separator2 = PredefinedMenuItem::separator(app)?;
        let file_menu = Submenu::with_items(app, "File", true, &[&new_tab, &new_window, &separator, &ssh_manager_menu, &separator2, &close_tab, &close_window])?;
        let edit_menu = Submenu::with_items(app, "Edit", true, &[
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ])?;

        let toggle_left_accel = config_key_to_accelerator(&keyboard.toggle_left_panel);
        let toggle_left = MenuItem::with_id(app, MENU_TOGGLE_LEFT_PANEL_ID, "Toggle File Explorer", true, Some(&toggle_left_accel))?;
        let toggle_right_accel = config_key_to_accelerator(&keyboard.toggle_right_panel);
        let toggle_right = MenuItem::with_id(app, MENU_TOGGLE_RIGHT_PANEL_ID, "Toggle Sessions Panel", true, Some(&toggle_right_accel))?;
        let toggle_bottom_accel = config_key_to_accelerator(&keyboard.toggle_bottom_panel);
        let toggle_bottom = MenuItem::with_id(app, "view.toggle_bottom_panel", "Toggle Bottom Panel", true, Some(&toggle_bottom_accel))?;
        let focus_sessions = MenuItem::with_id(app, MENU_FOCUS_SESSIONS_ID, "Toggle & Focus Sessions", true, Some("CmdOrCtrl+/"))?;
        let zen_accel = config_key_to_accelerator(&keyboard.zen_mode);
        let zen_mode = MenuItem::with_id(app, MENU_ZEN_MODE_ID, "Zen Mode", true, Some(&zen_accel))?;
        let zoom_in = MenuItem::with_id(app, MENU_ZOOM_IN_ID, "Zoom In", true, Some("CmdOrCtrl+="))?;
        let zoom_out = MenuItem::with_id(app, MENU_ZOOM_OUT_ID, "Zoom Out", true, Some("CmdOrCtrl+-"))?;
        let zoom_reset = MenuItem::with_id(app, MENU_ZOOM_RESET_ID, "Reset Zoom", true, Some("CmdOrCtrl+0"))?;
        let view_menu = Submenu::with_items(app, "View", true, &[
            &toggle_left, &toggle_right, &toggle_bottom,
            &PredefinedMenuItem::separator(app)?,
            &focus_sessions, &zen_mode,
            &PredefinedMenuItem::separator(app)?,
            &zoom_in, &zoom_out, &zoom_reset,
        ])?;

        let window_menu = Submenu::with_items(app, "Window", true, &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ])?;

        #[cfg(target_os = "macos")]
        {
            let app_name = app.package_info().name.clone();
            let app_menu = Submenu::with_items(app, app_name, true, &[
                &PredefinedMenuItem::about(app, None, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ])?;
            return Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &view_menu, &new_tools, &window_menu]);
        }

        #[cfg(not(target_os = "macos"))]
        {
            return Menu::with_items(app, &[&file_menu, &edit_menu, &view_menu, &new_tools, &window_menu]);
        }
    }

    Ok(base)
}

/// Return general app config the frontend needs.
#[tauri::command]
fn get_app_config(state: tauri::State<'_, TauriState>) -> serde_json::Value {
    let cfg = state.config.lock();
    let dec = format!("{:?}", cfg.window.decorations).to_lowercase();
    serde_json::json!({
        "appearance_mode": format!("{:?}", cfg.colors.appearance_mode).to_lowercase(),
        "zen_mode_shortcut": cfg.conch.keyboard.zen_mode,
        "decorations": dec,
        "platform": std::env::consts::OS,
    })
}

#[tauri::command]
fn get_home_dir() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string())
}

// ---------------------------------------------------------------------------
// Theme colors
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_theme_colors(state: tauri::State<'_, TauriState>) -> theme::ThemeColors {
    let cfg = state.config.lock();
    theme::resolve_theme_colors(&cfg)
}

// ---------------------------------------------------------------------------
// Terminal config (font, cursor, scroll)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct TerminalDisplayConfig {
    font_family: String,
    font_size: f64,
    cursor_style: String,
    cursor_blink: bool,
    scroll_sensitivity: f64,
}

#[tauri::command]
fn get_terminal_config(state: tauri::State<'_, TauriState>) -> TerminalDisplayConfig {
    let cfg = state.config.lock();
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

/// Convert a conch config keybinding (e.g. "cmd+shift+r") to a Tauri
/// accelerator string (e.g. "CmdOrCtrl+Shift+R").
fn config_key_to_accelerator(key: &str) -> String {
    key.split('+')
        .map(|part| {
            let lower = part.trim().to_lowercase();
            match lower.as_str() {
                "cmd" | "ctrl" => "CmdOrCtrl".to_string(),
                "shift" => "Shift".to_string(),
                "alt" | "opt" | "option" => "Alt".to_string(),
                other => other.to_uppercase(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Keyboard shortcuts exposed to the frontend.
#[derive(Serialize)]
struct KeyboardShortcuts {
    toggle_right_panel: String,
    toggle_left_panel: String,
    toggle_bottom_panel: String,
}

#[tauri::command]
fn get_keyboard_shortcuts(state: tauri::State<'_, TauriState>) -> KeyboardShortcuts {
    let cfg = state.config.lock();
    let kb = &cfg.conch.keyboard;
    KeyboardShortcuts {
        toggle_right_panel: kb.toggle_right_panel.clone(),
        toggle_left_panel: kb.toggle_left_panel.clone(),
        toggle_bottom_panel: kb.toggle_bottom_panel.clone(),
    }
}

// ---------------------------------------------------------------------------
// Window state persistence
// ---------------------------------------------------------------------------

/// Layout state sent from the frontend to persist.
#[derive(Deserialize)]
struct WindowLayout {
    ssh_panel_width: Option<f64>,
    ssh_panel_visible: Option<bool>,
    files_panel_width: Option<f64>,
    files_panel_visible: Option<bool>,
}

/// Layout state sent to the frontend on load.
#[derive(Serialize)]
struct SavedLayout {
    window_width: f64,
    window_height: f64,
    ssh_panel_width: f64,
    ssh_panel_visible: bool,
    files_panel_width: f64,
    files_panel_visible: bool,
}

#[tauri::command]
fn app_ready(window: tauri::WebviewWindow) {
    let _ = window.show();
}

#[tauri::command]
fn get_saved_layout() -> SavedLayout {
    let state = config::load_persistent_state().unwrap_or_default();
    SavedLayout {
        window_width: state.layout.window_width as f64,
        window_height: state.layout.window_height as f64,
        ssh_panel_width: state.layout.right_panel_width as f64,
        ssh_panel_visible: state.layout.right_panel_visible,
        files_panel_width: state.layout.left_panel_width as f64,
        files_panel_visible: state.layout.left_panel_visible,
    }
}

#[tauri::command]
fn save_window_layout(window: tauri::WebviewWindow, layout: WindowLayout) {
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
    let _ = config::save_persistent_state(&state);
}

#[tauri::command]
fn set_zoom_level(window: tauri::WebviewWindow, scale_factor: f64) -> Result<(), String> {
    window.set_zoom(scale_factor).map_err(|e| e.to_string())?;
    let mut state = config::load_persistent_state().unwrap_or_default();
    state.layout.zoom_factor = scale_factor as f32;
    let _ = config::save_persistent_state(&state);
    Ok(())
}

#[tauri::command]
fn get_zoom_level() -> f64 {
    let state = config::load_persistent_state().unwrap_or_default();
    let z = state.layout.zoom_factor as f64;
    if z > 0.0 { z } else { 1.0 }
}

pub(crate) fn build_app_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    keyboard: &conch_core::config::KeyboardConfig,
) -> tauri::Result<Menu<R>> {
    let new_tab = MenuItem::with_id(app, MENU_NEW_TAB_ID, "New Tab", true, Some("CmdOrCtrl+T"))?;
    let close_tab = MenuItem::with_id(
        app,
        MENU_CLOSE_TAB_ID,
        "Close Tab",
        true,
        Some("CmdOrCtrl+W"),
    )?;
    let new_window = MenuItem::with_id(
        app,
        MENU_NEW_WINDOW_ID,
        "New Window",
        true,
        Some("CmdOrCtrl+Shift+N"),
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let close_window = PredefinedMenuItem::close_window(app, None)?;

    let ssh_export = MenuItem::with_id(app, MENU_SSH_EXPORT_ID, "Export Connections", true, None::<&str>)?;
    let ssh_import = MenuItem::with_id(app, MENU_SSH_IMPORT_ID, "Import Connections", true, None::<&str>)?;
    let ssh_manager_menu = Submenu::with_items(app, "SSH Manager", true, &[&ssh_export, &ssh_import])?;
    let separator2 = PredefinedMenuItem::separator(app)?;
    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[&new_tab, &new_window, &separator, &ssh_manager_menu, &separator2, &close_tab, &close_window],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    // View menu — panel toggles using configured shortcuts
    let toggle_left_accel = config_key_to_accelerator(&keyboard.toggle_left_panel);
    let toggle_left = MenuItem::with_id(
        app,
        MENU_TOGGLE_LEFT_PANEL_ID,
        "Toggle File Explorer",
        true,
        Some(&toggle_left_accel),
    )?;
    let toggle_right_accel = config_key_to_accelerator(&keyboard.toggle_right_panel);
    let toggle_right = MenuItem::with_id(
        app,
        MENU_TOGGLE_RIGHT_PANEL_ID,
        "Toggle Sessions Panel",
        true,
        Some(&toggle_right_accel),
    )?;
    let focus_sessions = MenuItem::with_id(
        app,
        MENU_FOCUS_SESSIONS_ID,
        "Toggle & Focus Sessions",
        true,
        Some("CmdOrCtrl+/"),
    )?;
    let zen_accel = config_key_to_accelerator(&keyboard.zen_mode);
    let zen_mode = MenuItem::with_id(
        app,
        MENU_ZEN_MODE_ID,
        "Zen Mode",
        true,
        Some(&zen_accel),
    )?;
    let zoom_in = MenuItem::with_id(app, MENU_ZOOM_IN_ID, "Zoom In", true, Some("CmdOrCtrl+="))?;
    let zoom_out = MenuItem::with_id(app, MENU_ZOOM_OUT_ID, "Zoom Out", true, Some("CmdOrCtrl+-"))?;
    let zoom_reset = MenuItem::with_id(app, MENU_ZOOM_RESET_ID, "Reset Zoom", true, Some("CmdOrCtrl+0"))?;
    let toggle_bottom_accel = config_key_to_accelerator(&keyboard.toggle_bottom_panel);
    let toggle_bottom = MenuItem::with_id(app, "view.toggle_bottom_panel", "Toggle Bottom Panel", true, Some(&toggle_bottom_accel))?;
    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &toggle_left, &toggle_right, &toggle_bottom,
            &PredefinedMenuItem::separator(app)?,
            &focus_sessions, &zen_mode,
            &PredefinedMenuItem::separator(app)?,
            &zoom_in, &zoom_out, &zoom_reset,
        ],
    )?;

    let plugin_manager = MenuItem::with_id(
        app,
        MENU_PLUGIN_MANAGER_ID,
        "Plugin Manager\u{2026}",
        true,
        None::<&str>,
    )?;
    let manage_tunnels = MenuItem::with_id(
        app,
        MENU_MANAGE_TUNNELS_ID,
        "Manage SSH Tunnels\u{2026}",
        true,
        Some("CmdOrCtrl+Shift+T"),
    )?;
    let tools_menu = Submenu::with_items(
        app,
        "Tools",
        true,
        &[&plugin_manager, &PredefinedMenuItem::separator(app)?, &manage_tunnels],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    {
        let app_name = app.package_info().name.clone();
        let app_menu = Submenu::with_items(
            app,
            app_name,
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?;
        return Menu::with_items(
            app,
            &[&app_menu, &file_menu, &edit_menu, &view_menu, &tools_menu, &window_menu],
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        Menu::with_items(app, &[&file_menu, &edit_menu, &view_menu, &tools_menu, &window_menu])
    }
}

fn focused_webview_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Option<tauri::WebviewWindow<R>> {
    let windows = app.webview_windows();
    for window in windows.values() {
        if window.is_focused().unwrap_or(false) {
            return Some(window.clone());
        }
    }
    windows.into_values().next()
}

pub(crate) fn emit_menu_action_to_focused_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>, action: &str) {
    if let Some(window) = focused_webview_window(app) {
        let _ = window.emit(
            MENU_ACTION_EVENT,
            MenuActionEvent {
                window_label: window.label().to_string(),
                action: action.to_string(),
            },
        );
    }
}

/// Tauri command to open a new window (used by custom titlebar menu).
///
/// Window creation must happen on the main thread.  Tauri commands run on a
/// thread-pool, so we dispatch via `run_on_main_thread` to avoid a deadlock
/// (the builder's `build()` posts to the main thread and waits, but the main
/// thread may be blocked waiting for this command to finish).
#[tauri::command]
async fn open_new_window(app: tauri::AppHandle) -> Result<(), String> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if let Err(e) = create_new_window(&handle) {
            log::error!("Failed to create new window: {e}");
        }
    })
    .map_err(|e| e.to_string())
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
    let dec = if cfg!(target_os = "windows") { false } else { user_wants_dec };
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

/// Launch the Tauri-based UI.
pub fn run(config: UserConfig) -> anyhow::Result<()> {
    let (transfer_tx, mut transfer_rx) =
        tokio::sync::mpsc::unbounded_channel::<remote::transfer::TransferProgress>();
    let remote_state = Arc::new(Mutex::new(RemoteState::new(transfer_tx)));
    let plugins_config = config.conch.plugins.clone();
    let plugin_state = Arc::new(Mutex::new(plugins::PluginState::new(plugins_config.clone())));

    // Load persisted window size, falling back to config dimensions.
    let persisted = config::load_persistent_state().unwrap_or_default();
    let cfg_dims = &config.window.dimensions;
    let cfg_w = (cfg_dims.columns.max(80) as f64) * 8.0 + 40.0; // rough cell→pixel
    let cfg_h = (cfg_dims.lines.max(24) as f64) * 16.0 + 50.0;
    let initial_width = if persisted.layout.window_width > 100.0 {
        persisted.layout.window_width as f64
    } else {
        cfg_w.max(600.0)
    };
    let initial_height = if persisted.layout.window_height > 100.0 {
        persisted.layout.window_height as f64
    } else {
        cfg_h.max(400.0)
    };
    let user_wants_decorations = !matches!(
        config.window.decorations,
        conch_core::config::WindowDecorations::None
            | conch_core::config::WindowDecorations::Buttonless
    );
    // On Windows we always disable native decorations so we can render a
    // VS Code-style custom titlebar with inline menus.  On other platforms
    // we respect the user's decoration setting.
    let use_decorations = if cfg!(target_os = "windows") {
        false
    } else {
        user_wants_decorations
    };
    let window_theme = appearance_to_theme(&config.colors.appearance_mode);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(TauriState {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            config: Mutex::new(config),
        })
        .manage(Arc::clone(&remote_state))
        .manage(Arc::clone(&plugin_state))
        .setup(move |app| {
            let kb_config = config::load_user_config()
                .map(|c| c.conch.keyboard)
                .unwrap_or_default();
            let menu = build_app_menu(&app.handle(), &kb_config)
                .map_err(|e| anyhow::anyhow!("Failed to build app menu: {e}"))?;

            if cfg!(target_os = "windows") {
                // On Windows we use a custom titlebar with JS-driven menus
                // and accelerators.  Don't attach the native menu — it can
                // steal focus and interfere with shortcut handling.
            } else {
                app.handle()
                    .set_menu(menu)
                    .map_err(|e| anyhow::anyhow!("Failed to set app menu: {e}"))?;
            }

            // Apply persisted window size, decorations, theme, and zoom.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_size(tauri::LogicalSize::new(initial_width, initial_height));
                let _ = win.set_decorations(use_decorations);
                let _ = win.set_theme(window_theme);
                let zoom = persisted.layout.zoom_factor;
                if zoom > 0.0 && (zoom - 1.0).abs() > f32::EPSILON {
                    let _ = win.set_zoom(zoom as f64);
                }
            }

            // Initialize plugin system and restore previously enabled plugins.
            if plugins_config.enabled {
                let handle = app.handle().clone();
                let mut ps = plugin_state.lock();
                if plugins_config.java {
                    ps.init_java_manager(&handle);
                }
                // Restore plugins that were enabled in the previous session.
                ps.restore_plugins(&handle);
                drop(ps);

                // Rebuild the menu after a short delay to let plugin threads
                // run setup() and register their menu items.
                // On Windows, skip native menu rebuild (custom titlebar handles it).
                if !cfg!(target_os = "windows") {
                    let menu_handle = app.handle().clone();
                    let menu_kb = kb_config.clone();
                    let menu_ps = Arc::clone(&plugin_state);
                    std::thread::Builder::new()
                        .name("plugin-menu-rebuild".into())
                        .spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            let plugin_items = menu_ps.lock().menu_items.lock().clone();
                            if !plugin_items.is_empty() {
                                match build_app_menu_with_plugins(&menu_handle, &menu_kb, &plugin_items) {
                                    Ok(menu) => { let _ = menu_handle.set_menu(menu); }
                                    Err(e) => log::error!("Menu rebuild after plugin restore failed: {e}"),
                                }
                            }
                        })
                        .ok();
                }
            }

            // Start config/theme file watcher for hot-reload.
            watcher::start(app.handle().clone());

            // Start IPC socket listener.
            let _ipc_guard = ipc::start(app.handle().clone());
            // Keep the guard alive for the app's lifetime by leaking it.
            // The socket file is cleaned up on process exit.
            if let Some(guard) = _ipc_guard {
                std::mem::forget(guard);
            }

            // Forward transfer progress events to the frontend.
            // Use a std::thread since we're not inside a tokio runtime here.
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("transfer-progress".into())
                .spawn(move || {
                    while let Some(progress) = transfer_rx.blocking_recv() {
                        let _ = handle.emit("transfer-progress", &progress);
                    }
                })
                .ok();

            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_NEW_TAB_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_NEW_TAB),
            MENU_CLOSE_TAB_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_CLOSE_TAB),
            MENU_TOGGLE_LEFT_PANEL_ID => {
                emit_menu_action_to_focused_window(app, MENU_ACTION_TOGGLE_LEFT_PANEL)
            }
            MENU_ZEN_MODE_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_ZEN_MODE),
            MENU_ZOOM_IN_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_ZOOM_IN),
            MENU_ZOOM_OUT_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_ZOOM_OUT),
            MENU_ZOOM_RESET_ID => emit_menu_action_to_focused_window(app, MENU_ACTION_ZOOM_RESET),
            "view.toggle_bottom_panel" => emit_menu_action_to_focused_window(app, "toggle-bottom-panel"),
            MENU_TOGGLE_RIGHT_PANEL_ID => {
                emit_menu_action_to_focused_window(app, MENU_ACTION_TOGGLE_RIGHT_PANEL)
            }
            MENU_FOCUS_SESSIONS_ID => {
                emit_menu_action_to_focused_window(app, MENU_ACTION_FOCUS_SESSIONS)
            }
            MENU_PLUGIN_MANAGER_ID => {
                emit_menu_action_to_focused_window(app, MENU_ACTION_PLUGIN_MANAGER)
            }
            MENU_MANAGE_TUNNELS_ID => {
                emit_menu_action_to_focused_window(app, MENU_ACTION_MANAGE_TUNNELS)
            }
            MENU_SSH_EXPORT_ID => {
                emit_menu_action_to_focused_window(app, MENU_ACTION_SSH_EXPORT)
            }
            MENU_SSH_IMPORT_ID => {
                emit_menu_action_to_focused_window(app, MENU_ACTION_SSH_IMPORT)
            }
            MENU_NEW_WINDOW_ID => {
                if let Err(e) = create_new_window(app) {
                    log::error!("Failed to create window from menu: {e}");
                }
            }
            other => {
                // Check if it's a plugin menu item: "plugin.{source_name}.{action}"
                let id_str = other;
                if id_str.starts_with("plugin.") {
                    let parts: Vec<&str> = id_str.splitn(3, '.').collect();
                    if parts.len() == 3 {
                        let source_name = parts[1];
                        let action = parts[2].to_string();

                        if let Some(ps) = app.try_state::<Arc<Mutex<plugins::PluginState>>>() {
                            let ps_guard = ps.lock();
                            let bus = Arc::clone(&ps_guard.bus);

                            // Find the actual plugin name that registered this action.
                            // The source_name might be "java" (shared) while the real
                            // plugin name on the bus is different (e.g., "Form Test").
                            let real_plugin = ps_guard.menu_items.lock()
                                .iter()
                                .find(|i| i.plugin == source_name && i.action == action)
                                .map(|i| i.plugin.clone());

                            // Try direct match first, then all registered plugins.
                            let target = real_plugin.as_deref().unwrap_or(source_name);
                            let sent = if let Some(sender) = bus.sender_for(target) {
                                let event = conch_plugin_sdk::PluginEvent::MenuAction { action: action.clone() };
                                let json = serde_json::to_string(&event).unwrap_or_default();
                                sender.blocking_send(conch_plugin::bus::PluginMail::WidgetEvent { json }).is_ok()
                            } else {
                                false
                            };

                            // For Java plugins: the TauriHostApi name is "java" but
                            // plugins register on the bus with their actual name.
                            // Broadcast the action to all Java plugins if direct send failed.
                            if !sent {
                                let event = conch_plugin_sdk::PluginEvent::MenuAction { action };
                                let json = serde_json::to_string(&event).unwrap_or_default();
                                if let Some(ref mgr) = ps_guard.java_mgr {
                                    for meta in mgr.loaded_plugins() {
                                        if let Some(sender) = bus.sender_for(&meta.name) {
                                            let _ = sender.blocking_send(
                                                conch_plugin::bus::PluginMail::WidgetEvent { json: json.clone() }
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_ready,
            set_zoom_level,
            get_zoom_level,
            spawn_shell,
            write_to_pty,
            resize_pty,
            close_pty,
            current_window_label,
            get_saved_layout,
            save_window_layout,
            get_keyboard_shortcuts,
            get_theme_colors,
            get_terminal_config,
            get_app_config,
            get_home_dir,
            open_new_window,
            rebuild_menu,
            settings::get_all_settings,
            settings::save_settings,
            settings::list_themes,
            remote::ssh_connect,
            remote::ssh_quick_connect,
            remote::ssh_write,
            remote::ssh_resize,
            remote::ssh_disconnect,
            remote::remote_get_servers,
            remote::remote_save_server,
            remote::remote_delete_server,
            remote::remote_add_folder,
            remote::remote_delete_folder,
            remote::remote_import_ssh_config,
            remote::auth_respond_host_key,
            remote::auth_respond_password,
            remote::remote_get_sessions,
            remote::remote_rename_folder,
            remote::remote_set_folder_expanded,
            remote::remote_move_server,
            remote::remote_duplicate_server,
            remote::remote_export,
            remote::remote_import,
            remote::sftp_list_dir,
            remote::sftp_stat,
            remote::sftp_read_file,
            remote::sftp_write_file,
            remote::sftp_mkdir,
            remote::sftp_rename,
            remote::sftp_remove,
            remote::sftp_realpath,
            remote::local_list_dir,
            remote::local_stat,
            remote::local_mkdir,
            remote::local_rename,
            remote::local_remove,
            remote::transfer_download,
            remote::transfer_upload,
            remote::transfer_cancel,
            remote::tunnel_start,
            remote::tunnel_stop,
            remote::tunnel_save,
            remote::tunnel_delete,
            remote::tunnel_get_all,
            plugins::scan_plugins,
            plugins::enable_plugin,
            plugins::disable_plugin,
            plugins::dialog_respond_form,
            plugins::dialog_respond_prompt,
            plugins::dialog_respond_confirm,
            plugins::get_plugin_menu_items,
            plugins::trigger_plugin_menu_action,
            plugins::get_plugin_panels,
            plugins::get_panel_widgets,
            plugins::plugin_widget_event,
            plugins::request_plugin_render,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("Tauri error: {e}"))?;

    Ok(())
}

/// Continuously reads PTY output and emits "pty-output" events to the frontend.
fn pty_reader_loop(
    handle: &tauri::AppHandle,
    pty_state: &Arc<Mutex<HashMap<String, PtyBackend>>>,
    pty_key: String,
    window_label: String,
    tab_id: u32,
    mut reader: Box<dyn std::io::Read + Send>,
) {
    let mut buf = [0u8; 8192];
    let mut utf8 = crate::utf8_stream::Utf8Accumulator::new();

    loop {
        use std::io::Read;
        match reader.read(&mut buf) {
            Ok(0) => {
                // EOF — shell exited.
                pty_state.lock().remove(&pty_key);
                let _ = handle.emit_to(
                    &window_label,
                    "pty-exit",
                    PtyExitEvent {
                        window_label: window_label.clone(),
                        tab_id,
                    },
                );
                break;
            }
            Ok(n) => {
                let text = utf8.push(&buf[..n]);
                if text.is_empty() {
                    continue;
                }
                let _ = handle.emit_to(
                    &window_label,
                    "pty-output",
                    PtyOutputEvent {
                        window_label: window_label.clone(),
                        tab_id,
                        data: text,
                    },
                );
            }
            Err(e) => {
                log::error!("PTY read error on tab {tab_id}: {e}");
                pty_state.lock().remove(&pty_key);
                let _ = handle.emit_to(
                    &window_label,
                    "pty-exit",
                    PtyExitEvent {
                        window_label: window_label.clone(),
                        tab_id,
                    },
                );
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tauri_state_default_has_no_pty() {
        let state = TauriState {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            config: Mutex::new(UserConfig::default()),
        };
        assert!(state.ptys.lock().is_empty());
    }

    #[test]
    fn resolved_shell_empty_program_uses_default_shell() {
        let shell = conch_core::config::TerminalShell::default();
        let (program, args) = resolved_shell(&shell);
        assert!(program.is_none());
        assert!(args.is_empty());
    }

    #[test]
    fn resolved_shell_uses_configured_program_and_args() {
        let shell = conch_core::config::TerminalShell {
            program: "/bin/zsh".into(),
            args: vec!["-l".into(), "-c".into(), "echo ok".into()],
        };
        let (program, args) = resolved_shell(&shell);
        assert_eq!(program, Some("/bin/zsh"));
        assert_eq!(args, shell.args.as_slice());
    }
}
