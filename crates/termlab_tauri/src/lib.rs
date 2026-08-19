//! Tauri-based UI for TermLab (experimental).
//!
//! Uses xterm.js in a webview for terminal rendering, with a raw PTY backend
//! via `portable-pty`. This bypasses alacritty_terminal entirely — xterm.js
//! handles all terminal emulation.

pub(crate) mod chooser_window;
pub(crate) mod cleanup;
pub(crate) mod close_guard;
mod commands;
mod editor_fs;
pub(crate) mod extended_ansi;
pub(crate) mod font_metrics;
pub(crate) mod fonts;
mod ipc;
pub(crate) mod menu;
pub mod platform;
pub(crate) mod plugins;
pub(crate) mod pty;
mod pty_backend;
pub(crate) mod remote;
pub(crate) mod settings;
pub(crate) mod share_commands;
pub(crate) mod theme;
pub(crate) mod theme_catalog;
pub(crate) mod updater;
pub(crate) mod utf8_stream;
pub(crate) mod vault_commands;
mod watcher;
pub(crate) mod windows;

use std::collections::HashMap;
use std::sync::Arc;

use termlab_core::config::{self, UserConfig};
use parking_lot::{Mutex, RwLock};
use pty_backend::PtyBackend;
use remote::RemoteState;
use tauri::{Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

pub(crate) struct TauriState {
    ptys: Arc<Mutex<HashMap<String, PtyBackend>>>,
    active_panes: Arc<Mutex<HashMap<String, u32>>>,
    config: RwLock<UserConfig>,
    /// The user's home directory, used as a stable label for the app's
    /// "workspace" (see `commands::get_workspace_dir`). Captured once via
    /// `dirs::home_dir()` at startup — not the process's actual working
    /// directory — and never re-read, so it stays static for the window's
    /// lifetime even if a PTY's shell later `cd`s elsewhere.
    workspace_dir: Option<String>,
}

/// Launch the Tauri-based UI.
/// First approximation of the pixel size for a columns x lines setting.
///
/// Deliberately rough: the real cell size depends on the font, which only
/// exists inside the webview, so the frontend measures a fitted terminal and
/// corrects this to the exact cell count (app/core/window-size.js). This just
/// needs to be close enough that the correction is not a visible jump.
///
/// 0 columns/lines means "leave it to the system", which here means falling
/// back to a sensible default window rather than collapsing to nothing.
/// How long a window may stay hidden waiting for the frontend to size itself.
///
/// Generous on purpose: it exists to rescue a broken launch, not to race a
/// slow one. Firing early would reintroduce the visible resize it prevents.
const WINDOW_SHOW_FALLBACK_SECS: u64 = 5;

/// Show a window even if the frontend never asks us to.
///
/// Secondary windows (⌘⇧N) are created hidden and shown by `app_ready` once
/// the frontend is up. A frontend that dies before that call would leave a
/// window that never appears, and an app that looks like it failed to start
/// is worse than any cosmetic flaw — so this timer shows the window
/// regardless, and does nothing when the normal path already ran. The main
/// window is visible from creation and never needs it.
pub(crate) fn arm_window_show_fallback<R: tauri::Runtime>(app: &tauri::AppHandle<R>, label: &str) {
    let handle = app.clone();
    let label = label.to_string();
    std::thread::Builder::new()
        .name(format!("window-show-fallback-{label}"))
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(WINDOW_SHOW_FALLBACK_SECS));
            let Some(win) = handle.get_webview_window(&label) else {
                return; // window already closed — nothing to rescue
            };
            if matches!(win.is_visible(), Ok(false)) {
                log::warn!(
                    "window '{label}' was still hidden after {WINDOW_SHOW_FALLBACK_SECS}s; \
                     showing it without the configured size correction"
                );
                let _ = win.show();
            }
        })
        .ok();
}

pub(crate) fn estimate_window_px(dims: &termlab_core::config::WindowDimensions) -> (f64, f64) {
    const APPROX_CELL_W: f64 = 8.0;
    const APPROX_CELL_H: f64 = 16.0;
    const CHROME_W: f64 = 40.0;
    const CHROME_H: f64 = 50.0;
    if dims.columns == 0 || dims.lines == 0 {
        return (1200.0, 800.0);
    }
    let w = (dims.columns as f64) * APPROX_CELL_W + CHROME_W;
    let h = (dims.lines as f64) * APPROX_CELL_H + CHROME_H;
    (w.max(600.0), h.max(400.0))
}

/// The size a new window should open at, in logical pixels.
///
/// Nothing ever resizes a visible window; whatever this returns is what the
/// user gets, so the sources are ordered by how exactly they know the cell:
///
/// 1. The webview's own persisted measurement, when its font fingerprint
///    matches the current config — bit-exact, available from the second
///    launch under a given font.
/// 2. Native metrics parsed from the bundled font file (Alacritty's
///    approach) — exact cell, estimated chrome. Covers the first launch and
///    any launch after a font change, for the default font.
/// 3. The rough 8x16 estimate — only a user-chosen system font we cannot
///    parse, before its first measurement lands.
pub(crate) fn initial_window_px(
    dims: &termlab_core::config::WindowDimensions,
    metrics: &termlab_core::config::WindowMetrics,
    font: &termlab_core::config::FontConfig,
    zoom: f32,
) -> (f64, f64) {
    if dims.columns == 0 || dims.lines == 0 {
        return estimate_window_px(dims);
    }

    if metrics.is_usable() && metrics.matches(&font.normal.family, font.size, zoom) {
        let w = (dims.columns as f64) * (metrics.cell_width as f64) + metrics.chrome_width as f64;
        let h = (dims.lines as f64) * (metrics.cell_height as f64) + metrics.chrome_height as f64;
        return (w.max(600.0), h.max(400.0));
    }

    let zoom = if zoom > 0.0 { zoom as f64 } else { 1.0 };
    if let Some((cell_w, cell_h)) =
        font_metrics::default_font_cell(&font.normal.family, font.size as f64 * zoom)
    {
        // Chrome is estimated: the native path knows the cell exactly but not
        // the tab bar. Deliberately NOT clamped to the 600x400 floor the other
        // paths use — this size is already believable.
        const CHROME_W: f64 = 4.0;
        const CHROME_H: f64 = 44.0;
        let w = (dims.columns as f64) * cell_w + CHROME_W;
        let h = (dims.lines as f64) * cell_h + CHROME_H;
        return (w, h);
    }

    estimate_window_px(dims)
}

#[cfg(test)]
mod window_px_tests {
    use super::*;
    use termlab_core::config::{FontConfig, WindowDimensions, WindowMetrics};

    fn dims(columns: u16, lines: u16) -> WindowDimensions {
        WindowDimensions { columns, lines }
    }

    fn measured(family: &str, size: f32) -> WindowMetrics {
        WindowMetrics {
            cell_width: 9.6,
            cell_height: 21.0,
            chrome_width: 16.0,
            chrome_height: 64.0,
            font_family: family.to_string(),
            font_size: size,
            zoom: 1.0,
        }
    }

    #[test]
    fn matching_measurement_wins_outright() {
        let font = FontConfig::default(); // "JetBrains Mono", 14.0
        let m = measured(&font.normal.family, font.size);
        let (w, h) = initial_window_px(&dims(102, 46), &m, &font, 1.0);
        assert!((w - (102.0 * 9.6 + 16.0)).abs() < 0.001);
        assert!((h - (46.0 * 21.0 + 64.0)).abs() < 0.001);
    }

    #[test]
    fn a_stale_fingerprint_is_not_trusted() {
        // Metrics measured under a different size must not be applied — the
        // native path takes over instead, and its cell is 8.4 x 20 for the
        // bundled font at 14px (pinned in font_metrics tests).
        let font = FontConfig::default();
        let stale = measured(&font.normal.family, 16.0);
        let (w, h) = initial_window_px(&dims(110, 50), &stale, &font, 1.0);
        assert!((w - (110.0 * 8.4 + 4.0)).abs() < 0.001, "width was {w}");
        assert!((h - (50.0 * 20.0 + 44.0)).abs() < 0.001, "height was {h}");
    }

    #[test]
    fn a_pre_fingerprint_state_file_is_remeasured_not_trusted() {
        // Old state files deserialize with an empty family; that must never
        // match, so the native path decides.
        let font = FontConfig::default();
        let legacy = WindowMetrics {
            cell_width: 9.6,
            cell_height: 21.0,
            chrome_width: 16.0,
            chrome_height: 64.0,
            ..WindowMetrics::default()
        };
        let (w, _) = initial_window_px(&dims(110, 50), &legacy, &font, 1.0);
        assert!((w - (110.0 * 8.4 + 4.0)).abs() < 0.001, "width was {w}");
    }

    #[test]
    fn a_custom_font_without_measurement_falls_back_to_the_estimate() {
        let mut font = FontConfig::default();
        font.normal.family = "Menlo".to_string();
        assert_eq!(
            initial_window_px(&dims(102, 46), &WindowMetrics::default(), &font, 1.0),
            estimate_window_px(&dims(102, 46))
        );
    }

    #[test]
    fn zoom_scales_the_native_cell() {
        let font = FontConfig::default();
        let (w1, _) = initial_window_px(&dims(110, 50), &WindowMetrics::default(), &font, 1.0);
        let (w2, _) = initial_window_px(&dims(110, 50), &WindowMetrics::default(), &font, 2.0);
        // Double zoom doubles the grid, not the chrome.
        assert!(((w2 - 4.0) - 2.0 * (w1 - 4.0)).abs() < 0.001);
    }

    #[test]
    fn zero_dims_leave_it_to_the_system_regardless_of_metrics() {
        let font = FontConfig::default();
        let m = measured(&font.normal.family, font.size);
        assert_eq!(
            initial_window_px(&dims(0, 0), &m, &font, 1.0),
            (1200.0, 800.0)
        );
    }
}

pub fn run(config: UserConfig) -> anyhow::Result<()> {
    // Use the user's home directory as a stable "workspace" label rather than
    // the process's actual cwd (current_dir() titled the window after the
    // build directory) or a PTY's shell cwd (pty_backend spawns shells with
    // no explicit cwd, so where they actually start isn't tracked here).
    // Mirrors the reference app, whose title segment is its launch workspace.
    let workspace_dir = dirs::home_dir().map(|p| p.to_string_lossy().to_string());

    let (transfer_tx, mut transfer_rx) =
        tokio::sync::mpsc::unbounded_channel::<termlab_remote::transfer::TransferProgress>();
    let remote_state = Arc::new(Mutex::new(RemoteState::new(transfer_tx)));
    let plugins_config = config.termlab.plugins.clone();
    let plugin_state = Arc::new(Mutex::new(plugins::PluginState::new(
        plugins_config.clone(),
    )));

    let config_dir = config::config_dir();
    let vault_path = config_dir.join("vault.enc");
    let vault_state: vault_commands::VaultState =
        Arc::new(Mutex::new(termlab_vault::VaultManager::new(vault_path)));

    // Window size comes from the configured columns x lines, never from the
    // last session's pixels. Restoring the saved size made the setting inert
    // after first run, and meant a window spawned from a full-screen one opened
    // full-screen. What IS restored is the measured cell size and chrome from
    // the last run, so columns x lines converts to exact pixels here and the
    // window opens right the first frame (app/core/window-size.js keeps the
    // measurement fresh and corrects the one launch that has none).
    let persisted = config::load_persistent_state().unwrap_or_default();
    let cfg_dims = &config.window.dimensions;
    let (initial_width, initial_height) = initial_window_px(
        cfg_dims,
        &persisted.window_metrics,
        config.resolved_terminal_font(),
        persisted.layout.zoom_factor,
    );
    let user_wants_decorations = !matches!(
        config.window.decorations,
        termlab_core::config::WindowDecorations::None
            | termlab_core::config::WindowDecorations::Buttonless
    );
    // On Windows and Linux we disable native decorations so we can render a
    // VS Code-style custom titlebar with inline menus.  On Linux this avoids
    // the foreign-looking GTK menu bar on non-GNOME desktops (KDE, etc.).
    // On macOS we respect the user's decoration setting.
    let use_custom_titlebar = cfg!(target_os = "windows") || cfg!(target_os = "linux");
    let use_decorations = if use_custom_titlebar {
        false
    } else {
        user_wants_decorations
    };
    let window_theme = windows::appearance_to_theme(&config.colors.appearance_mode);

    log::info!("startup: window state loaded, building app");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .manage(TauriState {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            active_panes: Arc::new(Mutex::new(HashMap::new())),
            config: RwLock::new(config),
            workspace_dir,
        })
        .manage(Arc::clone(&remote_state))
        .manage(Arc::clone(&plugin_state))
        .manage(Arc::clone(&vault_state))
        .manage(updater::PendingUpdate::new())
        .manage(close_guard::CloseGuard::default())
        .manage(Mutex::new(chooser_window::ChooserRegistry::default()))
        .setup(move |app| {
            log::info!("startup: webview created, running app setup");
            let kb_config = config::load_user_config()
                .map(|c| c.termlab.keyboard)
                .unwrap_or_default();
            let the_menu = menu::build_app_menu(&app.handle(), &kb_config)
                .map_err(|e| anyhow::anyhow!("Failed to build app menu: {e}"))?;

            if cfg!(target_os = "windows") || cfg!(target_os = "linux") {
                // On Windows/Linux we use a custom titlebar with JS-driven
                // menus and accelerators.  Don't attach the native menu — it
                // can steal focus and interfere with shortcut handling.
            } else {
                app.handle()
                    .set_menu(the_menu)
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

            arm_window_show_fallback(app.handle(), "main");

            // Initialize plugin system and restore previously enabled plugins.
            if plugins_config.enabled {
                let handle = app.handle().clone();
                let mut ps = plugin_state.lock();
                if plugins_config.java {
                    ps.init_java_manager(&handle);
                }
                // Restore plugins that were enabled in the previous session.
                ps.restore_plugins(&handle);
                let bus = Arc::clone(&ps.bus);
                drop(ps);

                // Publish a lightweight host tick event for plugins that need
                // polling/synchronization with external state changes.
                tauri::async_runtime::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                    loop {
                        ticker.tick().await;
                        let unix_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        bus.publish(
                            "__host__",
                            "host.tick",
                            serde_json::json!({ "unix_ms": unix_ms }),
                        );
                    }
                });

                // Rebuild the menu after a short delay to let plugin threads
                // run setup() and register their menu items.
                // On Windows/Linux, skip native menu rebuild (custom titlebar handles it).
                if !(cfg!(target_os = "windows") || cfg!(target_os = "linux")) {
                    let menu_handle = app.handle().clone();
                    let menu_kb = kb_config.clone();
                    let menu_ps = Arc::clone(&plugin_state);
                    std::thread::Builder::new()
                        .name("plugin-menu-rebuild".into())
                        .spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            let plugin_items = menu_ps.lock().menu_items.read().clone();
                            if !plugin_items.is_empty() {
                                match menu::build_app_menu_with_plugins(
                                    &menu_handle,
                                    &menu_kb,
                                    &plugin_items,
                                ) {
                                    Ok(new_menu) => {
                                        let _ = menu_handle.set_menu(new_menu);
                                    }
                                    Err(e) => {
                                        log::error!("Menu rebuild after plugin restore failed: {e}")
                                    }
                                }
                            }
                        })
                        .ok();
                }
            }

            // Start theme file watcher for hot-reload.
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

            // Sweep orphaned light-editor temp files left by a previous crash.
            // Uses a std::thread since we're not inside a tokio runtime here.
            std::thread::Builder::new()
                .name("editor-temp-sweep".into())
                .spawn(|| {
                    let _ = editor_fs::editor_temp_sweep();
                })
                .ok();

            // Vault auto-lock background checker (every 30 seconds).
            // Uses a std::thread since we're not inside a tokio runtime here.
            {
                let vault_for_timer = Arc::clone(&vault_state);
                let app_for_timer = app.handle().clone();
                std::thread::Builder::new()
                    .name("vault-auto-lock".into())
                    .spawn(move || {
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(30));
                            let did_lock = vault_for_timer.lock().check_timeout();
                            if did_lock {
                                let _ = app_for_timer.emit("vault-locked", ());
                            }
                        }
                    })
                    .ok();
            }

            // Check whether a legacy-to-vault migration is needed.
            // If the vault file does not exist yet AND servers.json has legacy entries
            // (plain-text user/auth fields without a vault_account_id), notify the
            // frontend so it can prompt the user to set up the vault and migrate.
            {
                let vault_exists = vault_state.lock().vault_exists();
                if !vault_exists {
                    let has_legacy = remote_state.lock().config.has_legacy_entries();
                    if has_legacy {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.emit("vault-migration-needed", ());
                        }
                    }
                }
            }

            // Auto-check for updates on startup (macOS/Windows only)
            if cfg!(not(target_os = "linux")) {
                let check_enabled = termlab_core::config::load_user_config()
                    .map(|c| c.termlab.check_for_updates)
                    .unwrap_or(true);
                if check_enabled {
                    let app_handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        let update = match app_handle.updater() {
                            Ok(u) => u.check().await,
                            Err(e) => {
                                log::warn!("Startup updater init failed: {e}");
                                return;
                            }
                        };
                        match update {
                            Ok(Some(update)) => {
                                let info = updater::UpdateInfo {
                                    version: update.version.clone(),
                                    body: update.body.clone(),
                                };
                                let pending = app_handle.state::<updater::PendingUpdate>();
                                *pending.0.lock() = Some(update);
                                let _ = app_handle.emit("update-available", &info);
                            }
                            Ok(None) => log::debug!("No updates available"),
                            Err(e) => log::warn!("Startup update check failed: {e}"),
                        }
                    });
                }
            }

            log::info!("startup: app setup complete");
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            // Not PredefinedMenuItem::quit — see menu::MENU_QUIT_ID. Every
            // armed window gets asked about unsaved editors before the exit.
            menu::MENU_QUIT_ID => close_guard::request_quit(app),
            menu::MENU_NEW_TAB_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_NEW_TAB)
            }
            menu::MENU_NEW_PLAIN_SHELL_TAB_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_NEW_PLAIN_SHELL_TAB)
            }
            menu::MENU_CLOSE_TAB_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_CLOSE_TAB)
            }
            menu::MENU_RENAME_TAB_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_RENAME_TAB)
            }
            menu::MENU_NEW_FILE_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_NEW_FILE)
            }
            menu::MENU_OPEN_FILE_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_OPEN_FILE)
            }
            menu::MENU_SAVE_FILE_AS_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_SAVE_FILE_AS)
            }
            menu::MENU_TOGGLE_LEFT_PANEL_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_TOGGLE_LEFT_PANEL)
            }
            menu::MENU_ZEN_MODE_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_ZEN_MODE)
            }
            menu::MENU_ZOOM_IN_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_ZOOM_IN)
            }
            menu::MENU_ZOOM_OUT_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_ZOOM_OUT)
            }
            menu::MENU_ZOOM_RESET_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_ZOOM_RESET)
            }
            menu::MENU_TOGGLE_BOTTOM_PANEL_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_TOGGLE_BOTTOM_PANEL)
            }
            menu::MENU_TOGGLE_RIGHT_PANEL_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_TOGGLE_RIGHT_PANEL)
            }
            menu::MENU_FOCUS_SESSIONS_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_FOCUS_SESSIONS)
            }
            menu::MENU_SETTINGS_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_SETTINGS)
            }
            menu::MENU_MANAGE_TUNNELS_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_MANAGE_TUNNELS)
            }
            menu::MENU_SSH_EXPORT_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_SSH_EXPORT)
            }
            menu::MENU_SSH_IMPORT_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_SSH_IMPORT)
            }
            menu::MENU_VAULT_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_VAULT_OPEN)
            }
            menu::MENU_KEYGEN_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_KEYGEN_OPEN)
            }
            menu::MENU_VAULT_LOCK_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_VAULT_LOCK)
            }
            menu::MENU_CHECK_UPDATES_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_CHECK_UPDATES)
            }
            menu::MENU_ABOUT_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_ABOUT)
            }
            menu::MENU_OPEN_DEVTOOLS_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_OPEN_DEVTOOLS)
            }
            menu::MENU_SPLIT_VERTICAL_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_SPLIT_VERTICAL)
            }
            menu::MENU_SPLIT_HORIZONTAL_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_SPLIT_HORIZONTAL)
            }
            menu::MENU_CLOSE_PANE_ID => {
                menu::emit_menu_action_to_focused_window(app, menu::MENU_ACTION_CLOSE_PANE)
            }
            menu::MENU_NEW_WINDOW_ID => {
                if let Err(e) = windows::create_new_window(app) {
                    log::error!("Failed to create window from menu: {e}");
                }
            }
            other => {
                // Check if it's a plugin menu item: "plugin.{source_name}.{action}"
                let id_str = other;
                if id_str.starts_with("plugin.") {
                    if let Some(ps) = app.try_state::<Arc<Mutex<plugins::PluginState>>>() {
                        let ps_guard = ps.lock();
                        let bus = Arc::clone(&ps_guard.bus);
                        let mut target_plugin: Option<String> = None;
                        let mut action: Option<String> = None;

                        // Resolve by exact menu-id match so plugin names and actions
                        // can safely contain '.'.
                        {
                            let items = ps_guard.menu_items.read();
                            if let Some(item) = items
                                .iter()
                                .find(|i| format!("plugin.{}.{}", i.plugin, i.action) == id_str)
                            {
                                target_plugin = Some(item.plugin.clone());
                                action = Some(item.action.clone());
                            }
                        }

                        // Backward-compatible fallback for legacy IDs.
                        if target_plugin.is_none() || action.is_none() {
                            let parts: Vec<&str> = id_str.splitn(3, '.').collect();
                            if parts.len() == 3 {
                                target_plugin = Some(parts[1].to_string());
                                action = Some(parts[2].to_string());
                            }
                        }

                        let Some(action) = action else {
                            return;
                        };
                        let target = target_plugin.as_deref().unwrap_or_default();
                        let sent = if let Some(sender) = bus.sender_for(target) {
                            let event = termlab_plugin_sdk::PluginEvent::MenuAction {
                                action: action.clone(),
                            };
                            let json = serde_json::to_string(&event).unwrap_or_default();
                            sender
                                .blocking_send(termlab_plugin::bus::PluginMail::WidgetEvent { json })
                                .is_ok()
                        } else {
                            false
                        };

                        // For Java plugins: the TauriHostApi name can be shared while
                        // plugins register on the bus with their own names.
                        if !sent {
                            let event = termlab_plugin_sdk::PluginEvent::MenuAction { action };
                            let json = serde_json::to_string(&event).unwrap_or_default();
                            if let Some(ref mgr) = ps_guard.java_mgr {
                                for meta in mgr.loaded_plugins() {
                                    if let Some(sender) = bus.sender_for(&meta.name) {
                                        let _ = sender.blocking_send(
                                            termlab_plugin::bus::PluginMail::WidgetEvent {
                                                json: json.clone(),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
        .on_window_event(|window, event| {
            // Stop the close and ask the webview about unsaved editors. It
            // answers by calling `confirm_window_close`, which retries the
            // close with permission in hand (see close_guard.rs).
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let ask_first = close_guard::on_close_requested(window);
                if ask_first {
                    api.prevent_close();
                }
                // A chooser window's own close button (its native close, not
                // the parent's) is Cancel. Does not prevent the close — the
                // window is allowed to go away on its own; this only does the
                // registry cleanup, the emit, and the size persistence while
                // the window is still alive to read from.
                chooser_window::on_chooser_close_requested(window);
            }

            // IntelliJ-style modal focus: clicking the main window while
            // the settings window is open redirects focus to settings.
            if let tauri::WindowEvent::Focused(true) = event {
                if window.label() != "settings" {
                    if let Some(settings_win) = window.app_handle().get_webview_window("settings") {
                        let _ = settings_win.set_focus();
                    }
                }
                // Same idea for a chooser: focusing its parent bounces focus
                // back to the chooser (the modal focus bounce, spec "Window &
                // lifecycle" — works uniformly including Linux, where
                // `.parent()` is best-effort).
                chooser_window::on_window_focused(window);
            }

            if let tauri::WindowEvent::Destroyed = event {
                let label = window.label().to_string();
                log::info!("Window '{label}' destroyed — starting cleanup");

                // Drop this label's close permission so it cannot be inherited
                // by a future window that happens to reuse the name, and keep
                // a quit poll moving if it was waiting on this window.
                close_guard::on_window_destroyed(window);

                // If this window was some chooser's parent, that chooser is
                // now orphaned: resolve it as cancelled and close it. The
                // `.parent()` owner relationship (macOS/Windows) is not
                // trusted alone to do this on every platform.
                chooser_window::on_window_destroyed(window);

                // When the main window closes, also close child windows
                // (settings, etc.) so they don't linger as orphans.
                if label == "main" {
                    if let Some(settings_win) = window.app_handle().get_webview_window("settings") {
                        let _ = settings_win.close();
                    }
                }

                // Clean up PTY sessions for this window.
                if let Some(state) = window.try_state::<TauriState>() {
                    let pty_count = cleanup::cleanup_ptys(&state.ptys, &label);
                    if pty_count > 0 {
                        log::info!("Cleaned up {pty_count} PTY session(s) for window '{label}'");
                    }
                }

                // Clean up SSH sessions for this window.
                if let Some(remote) = window.try_state::<Arc<Mutex<RemoteState>>>() {
                    let ssh_count = cleanup::cleanup_ssh_sessions(&remote, &label);
                    if ssh_count > 0 {
                        log::info!("Cleaned up {ssh_count} SSH session(s) for window '{label}'");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_ready,
            commands::open_devtools,
            commands::set_zoom_level,
            commands::save_window_metrics,
            commands::get_zoom_level,
            pty::spawn_shell,
            pty::spawn_default_shell,
            pty::write_to_pty,
            pty::resize_pty,
            pty::close_pty,
            pty::get_local_pane_cwd,
            pty::get_local_pane_process,
            pty::get_host_identity,
            commands::current_window_label,
            commands::set_active_pane,
            commands::get_saved_layout,
            commands::save_window_layout,
            commands::get_keyboard_shortcuts,
            commands::get_theme_colors,
            commands::get_terminal_config,
            commands::get_app_config,
            commands::get_about_info,
            commands::get_home_dir,
            commands::get_workspace_dir,
            commands::clipboard_read_text,
            commands::clipboard_write_text,
            windows::open_new_window,
            windows::open_settings_window,
            chooser_window::open_file_chooser,
            chooser_window::get_chooser_request,
            chooser_window::resolve_file_chooser,
            chooser_window::cancel_file_chooser,
            chooser_window::chooser_ready,
            chooser_window::focus_file_chooser,
            commands::rebuild_menu,
            settings::get_all_settings,
            settings::save_settings,
            settings::list_themes,
            settings::preview_theme_colors,
            theme_catalog::list_terminal_themes,
            fonts::list_system_fonts,
            remote::ssh_commands::ssh_connect,
            remote::ssh_commands::ssh_quick_connect,
            remote::ssh_commands::ssh_write,
            remote::ssh_commands::ssh_resize,
            remote::ssh_commands::ssh_disconnect,
            remote::ssh_commands::ssh_get_pane_cwd,
            remote::ssh_commands::ssh_open_channel,
            remote::server_commands::remote_get_servers,
            remote::server_commands::remote_save_server,
            remote::server_commands::remote_delete_server,
            remote::server_commands::remote_add_folder,
            remote::server_commands::remote_delete_folder,
            remote::server_commands::remote_import_ssh_config,
            remote::auth::auth_respond_host_key,
            remote::auth::auth_respond_password,
            remote::server_commands::remote_get_sessions,
            remote::server_commands::remote_rename_folder,
            remote::server_commands::remote_set_folder_expanded,
            remote::server_commands::remote_move_server,
            remote::server_commands::remote_duplicate_server,
            share_commands::share_export_preview,
            share_commands::share_export,
            share_commands::share_pick_import_file,
            share_commands::share_import_plan,
            share_commands::share_import_apply,
            remote::sftp_commands::sftp_list_dir,
            remote::sftp_commands::sftp_stat,
            remote::sftp_commands::sftp_read_file,
            remote::sftp_commands::sftp_write_file,
            remote::sftp_commands::sftp_mkdir,
            remote::sftp_commands::sftp_rename,
            remote::sftp_commands::sftp_remove,
            remote::sftp_commands::sftp_realpath,
            remote::sftp_commands::local_list_dir,
            remote::sftp_commands::local_stat,
            remote::sftp_commands::local_mkdir,
            remote::sftp_commands::local_rename,
            remote::sftp_commands::local_remove,
            remote::transfer_commands::transfer_download,
            remote::transfer_commands::transfer_upload,
            remote::transfer_commands::transfer_cancel,
            remote::tunnel_commands::tunnel_start,
            remote::tunnel_commands::tunnel_stop,
            remote::tunnel_commands::tunnel_save,
            remote::tunnel_commands::tunnel_delete,
            remote::tunnel_commands::tunnel_get_all,
            plugins::scan_plugins,
            plugins::enable_plugin,
            plugins::disable_plugin,
            plugins::dialog_respond_form,
            plugins::dialog_respond_prompt,
            plugins::dialog_respond_confirm,
            plugins::plugin_respond_new_tab,
            plugins::get_plugin_menu_items,
            plugins::trigger_plugin_menu_action,
            plugins::get_plugin_panels,
            plugins::get_panel_widgets,
            plugins::get_plugin_settings_sections,
            plugins::commit_plugin_settings_drafts,
            plugins::discard_plugin_settings_drafts,
            plugins::register_plugin_view_binding,
            plugins::plugin_view_closed,
            plugins::plugin_widget_event,
            plugins::request_plugin_render,
            plugins::request_plugin_view_render,
            vault_commands::vault_status,
            vault_commands::vault_create,
            vault_commands::vault_unlock,
            vault_commands::vault_lock,
            vault_commands::vault_list_accounts,
            vault_commands::vault_get_account,
            vault_commands::vault_add_account,
            vault_commands::vault_update_account,
            vault_commands::vault_delete_account,
            vault_commands::vault_get_settings,
            vault_commands::vault_update_settings,
            vault_commands::vault_pick_key_file,
            vault_commands::vault_check_path_exists,
            vault_commands::vault_generate_key,
            vault_commands::vault_list_keys,
            vault_commands::vault_delete_key,
            vault_commands::vault_migrate_legacy,
            updater::check_for_update,
            updater::install_update,
            updater::restart_app,
            editor_fs::editor_can_open,
            editor_fs::editor_read_file,
            editor_fs::editor_write_file,
            editor_fs::editor_temp_path,
            editor_fs::editor_temp_cleanup,
            // editor_temp_sweep is deliberately absent, and is no longer a
            // #[tauri::command] at all: it deletes the entire remote-edit temp
            // root, which would destroy the backing file of every open remote
            // editor. Both its callers are Rust — the setup hook above and
            // close_guard::finish_exit — and neither runs with an editor live.
            close_guard::window_close_guard_arm,
            close_guard::confirm_window_close,
            close_guard::quit_vote,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("Tauri error: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tauri_state_default_has_no_pty() {
        let state = TauriState {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            active_panes: Arc::new(Mutex::new(HashMap::new())),
            config: RwLock::new(UserConfig::default()),
            workspace_dir: None,
        };
        assert!(state.ptys.lock().is_empty());
    }
}
