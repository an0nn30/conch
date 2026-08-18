//! Tauri-based UI for TermLab (experimental).
//!
//! Uses xterm.js in a webview for terminal rendering, with a raw PTY backend
//! via `portable-pty`. This bypasses alacritty_terminal entirely — xterm.js
//! handles all terminal emulation.

pub(crate) mod cleanup;
pub(crate) mod close_guard;
mod commands;
mod editor_fs;
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
    // full-screen. This is only a first approximation — real cell metrics live
    // in the webview, so the frontend corrects it to the exact cell count once
    // the terminal has measured itself (app/core/window-size.js).
    let cfg_dims = &config.window.dimensions;
    let (initial_width, initial_height) = estimate_window_px(cfg_dims);
    // Still needed for zoom, which is genuinely a restored preference.
    let persisted = config::load_persistent_state().unwrap_or_default();
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
            }

            // IntelliJ-style modal focus: clicking the main window while
            // the settings window is open redirects focus to settings.
            if let tauri::WindowEvent::Focused(true) = event {
                if window.label() != "settings" {
                    if let Some(settings_win) = window.app_handle().get_webview_window("settings") {
                        let _ = settings_win.set_focus();
                    }
                }
            }

            if let tauri::WindowEvent::Destroyed = event {
                let label = window.label().to_string();
                log::info!("Window '{label}' destroyed — starting cleanup");

                // Drop this label's close permission so it cannot be inherited
                // by a future window that happens to reuse the name, and keep
                // a quit poll moving if it was waiting on this window.
                close_guard::on_window_destroyed(window);

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
            commands::rebuild_menu,
            settings::get_all_settings,
            settings::save_settings,
            settings::list_themes,
            settings::preview_theme_colors,
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
            editor_fs::editor_scratch_dir,
            editor_fs::editor_scratch_list,
            editor_fs::editor_temp_path,
            editor_fs::editor_temp_cleanup,
            // editor_temp_sweep is deliberately NOT registered: it deletes the
            // entire remote-edit temp root, which would destroy the backing
            // file of every open remote editor. Its only callers are Rust —
            // startup (above) and finish_exit — where no editor is live.
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
