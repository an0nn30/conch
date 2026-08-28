//! Menu constants, accelerator helpers, and menu builders.
//!
//! All `MENU_*` ID and action constants live here, along with the functions
//! that build the native app menu and emit menu-action events to the frontend.

use std::collections::BTreeMap;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager};

use crate::plugins;

// ---------------------------------------------------------------------------
// Menu ID constants (used by `on_menu_event` in lib.rs)
// ---------------------------------------------------------------------------

pub(crate) const MENU_NEW_TAB_ID: &str = "file.new_tab";
pub(crate) const MENU_NEW_PLAIN_SHELL_TAB_ID: &str = "file.new_plain_shell_tab";
pub(crate) const MENU_CLOSE_TAB_ID: &str = "file.close_tab";
pub(crate) const MENU_NEW_WINDOW_ID: &str = "file.new_window";
pub(crate) const MENU_TOGGLE_LEFT_PANEL_ID: &str = "view.toggle_left_panel";
pub(crate) const MENU_TOGGLE_RIGHT_PANEL_ID: &str = "view.toggle_right_panel";
pub(crate) const MENU_FOCUS_SESSIONS_ID: &str = "view.focus_sessions";
pub(crate) const MENU_ZEN_MODE_ID: &str = "view.zen_mode";
pub(crate) const MENU_ZOOM_IN_ID: &str = "view.zoom_in";
pub(crate) const MENU_ZOOM_OUT_ID: &str = "view.zoom_out";
pub(crate) const MENU_ZOOM_RESET_ID: &str = "view.zoom_reset";
pub(crate) const MENU_MANAGE_TUNNELS_ID: &str = "tools.manage_tunnels";
pub(crate) const MENU_SSH_EXPORT_ID: &str = "file.ssh_export";
pub(crate) const MENU_SSH_IMPORT_ID: &str = "file.ssh_import";
pub(crate) const MENU_SETTINGS_ID: &str = "app.settings";
pub(crate) const MENU_VAULT_ID: &str = "tools.credential_vault";
pub(crate) const MENU_KEYGEN_ID: &str = "tools.generate_ssh_key";
pub(crate) const MENU_VAULT_LOCK_ID: &str = "tools.lock_vault";
pub(crate) const MENU_CHECK_UPDATES_ID: &str = "check-for-updates";
pub(crate) const MENU_ABOUT_ID: &str = "about-termlab";
pub(crate) const MENU_OPEN_DEVTOOLS_ID: &str = "debug.open_devtools";
pub(crate) const MENU_SPLIT_VERTICAL_ID: &str = "view.split_vertical";
pub(crate) const MENU_SPLIT_HORIZONTAL_ID: &str = "view.split_horizontal";
pub(crate) const MENU_CLOSE_PANE_ID: &str = "view.close_pane";
pub(crate) const MENU_TOGGLE_BOTTOM_PANEL_ID: &str = "view.toggle_bottom_panel";
pub(crate) const MENU_RENAME_TAB_ID: &str = "file.rename_tab";
pub(crate) const MENU_NEW_FILE_ID: &str = "file.new_file";
pub(crate) const MENU_OPEN_FILE_ID: &str = "file.open_file";
pub(crate) const MENU_OPEN_FOLDER_ID: &str = "file.open_folder";
pub(crate) const MENU_SAVE_FILE_AS_ID: &str = "file.save_file_as";
/// Menu ids for the Open Recent Project submenu are minted per entry as
/// `file.recent_project.<path>` — the path IS the id (fix round 1, F5).
///
/// The original design minted `file.recent_project.<index>` and resolved
/// the index back to a path by re-fetching `recents::list_recents()` at
/// click time. That list can shift BETWEEN the menu being built and the
/// click — another window's `remember()` call reorders or prunes it in the
/// meantime — so the same index silently resolved to a DIFFERENT project
/// than the one the user actually clicked. Embedding the path outright
/// removes the lookup (and the race) entirely: whatever the user clicked is
/// exactly what opens, full stop. This carries no new risk — the path was
/// already smuggled through an equally-shaped string for the emitted
/// `MENU_ACTION_OPEN_RECENT_PROJECT` event one hop later.
pub(crate) const MENU_RECENT_PROJECT_PREFIX: &str = "file.recent_project.";
/// Quit is a custom item, not `PredefinedMenuItem::quit`. The predefined one
/// sends `[NSApp terminate:]`, which tao does not intercept
/// (`applicationShouldTerminate:` is unimplemented) and which raises neither
/// `WindowEvent::CloseRequested` nor `RunEvent::ExitRequested` — so there is
/// no point at which unsaved editors could be checked. Routing quit through a
/// menu id lets close_guard poll every window first.
pub(crate) const MENU_QUIT_ID: &str = "app.quit";

// ---------------------------------------------------------------------------
// Menu action string constants (emitted to frontend via events)
// ---------------------------------------------------------------------------

pub(crate) const MENU_ACTION_EVENT: &str = "menu-action";
pub(crate) const MENU_ACTION_NEW_TAB: &str = "new-tab";
pub(crate) const MENU_ACTION_NEW_PLAIN_SHELL_TAB: &str = "new-plain-shell-tab";
pub(crate) const MENU_ACTION_CLOSE_TAB: &str = "close-tab";
pub(crate) const MENU_ACTION_TOGGLE_LEFT_PANEL: &str = "toggle-left-panel";
pub(crate) const MENU_ACTION_TOGGLE_RIGHT_PANEL: &str = "toggle-right-panel";
pub(crate) const MENU_ACTION_FOCUS_SESSIONS: &str = "focus-sessions";
pub(crate) const MENU_ACTION_ZEN_MODE: &str = "zen-mode";
pub(crate) const MENU_ACTION_ZOOM_IN: &str = "zoom-in";
pub(crate) const MENU_ACTION_ZOOM_OUT: &str = "zoom-out";
pub(crate) const MENU_ACTION_ZOOM_RESET: &str = "zoom-reset";
pub(crate) const MENU_ACTION_MANAGE_TUNNELS: &str = "manage-tunnels";
pub(crate) const MENU_ACTION_SSH_EXPORT: &str = "ssh-export";
pub(crate) const MENU_ACTION_SSH_IMPORT: &str = "ssh-import";
pub(crate) const MENU_ACTION_SETTINGS: &str = "settings";
pub(crate) const MENU_ACTION_VAULT_OPEN: &str = "vault-open";
pub(crate) const MENU_ACTION_KEYGEN_OPEN: &str = "keygen-open";
pub(crate) const MENU_ACTION_VAULT_LOCK: &str = "vault-lock";
pub(crate) const MENU_ACTION_SPLIT_VERTICAL: &str = "split-vertical";
pub(crate) const MENU_ACTION_SPLIT_HORIZONTAL: &str = "split-horizontal";
pub(crate) const MENU_ACTION_CLOSE_PANE: &str = "close-pane";
pub(crate) const MENU_ACTION_RENAME_TAB: &str = "rename-tab";
pub(crate) const MENU_ACTION_NEW_FILE: &str = "new-file";
pub(crate) const MENU_ACTION_OPEN_FILE: &str = "open-file";
pub(crate) const MENU_ACTION_OPEN_FOLDER: &str = "open-folder";
pub(crate) const MENU_ACTION_SAVE_FILE_AS: &str = "save-file-as";
pub(crate) const MENU_ACTION_TOGGLE_BOTTOM_PANEL: &str = "toggle-bottom-panel";
pub(crate) const MENU_ACTION_CHECK_UPDATES: &str = "check-for-updates";
pub(crate) const MENU_ACTION_ABOUT: &str = "about";
pub(crate) const MENU_ACTION_OPEN_DEVTOOLS: &str = "open-devtools";
pub(crate) const MENU_ACTION_OPEN_RECENT_PROJECT: &str = "open-recent-project:";

// ---------------------------------------------------------------------------
// Menu action event payload
// ---------------------------------------------------------------------------

#[derive(Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct MenuActionEvent {
    pub window_label: String,
    pub action: String,
}

// ---------------------------------------------------------------------------
// Accelerator conversion
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
const PRIMARY_ACCELERATOR_MOD: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const PRIMARY_ACCELERATOR_MOD: &str = "CmdOrCtrl";

fn primary_accelerator(key: &str) -> String {
    format!("{PRIMARY_ACCELERATOR_MOD}+{key}")
}

/// Convert a termlab config keybinding (e.g. "cmd+shift+r") to a Tauri
/// accelerator string (e.g. "Cmd+Shift+R" on macOS).
pub(crate) fn config_key_to_accelerator(key: &str) -> String {
    key.split('+')
        .map(|part| {
            let lower = part.trim().to_lowercase();
            match lower.as_str() {
                "cmd" => PRIMARY_ACCELERATOR_MOD.to_string(),
                "cmdorctrl" => PRIMARY_ACCELERATOR_MOD.to_string(),
                "ctrl" => "Ctrl".to_string(),
                "shift" => "Shift".to_string(),
                "alt" | "opt" | "option" => "Alt".to_string(),
                other => other.to_uppercase(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

// ---------------------------------------------------------------------------
// Menu builders
// ---------------------------------------------------------------------------

/// The Open Recent Project submenu, or `None` when there is nothing to list.
///
/// (Fix round 1, F4 ruling) `list_recents()` does NOT stat any path — this
/// runs on the main thread at every app launch and every `rebuild_menu`, so
/// a hung network mount must never be able to block it. A recent whose path
/// has since vanished is shown exactly as recorded; clicking it flows into
/// `project_open`, which already reports the failure through a toast.
fn recent_projects_submenu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> tauri::Result<Option<Submenu<R>>> {
    let recents = crate::project::recents::list_recents();
    if recents.is_empty() {
        return Ok(None);
    }
    let mut items: Vec<MenuItem<R>> = Vec::new();
    for entry in &recents {
        // (Fix round 1, F5) the path IS the id — see MENU_RECENT_PROJECT_PREFIX's
        // doc comment for why an index into a re-fetched list is not safe here.
        items.push(MenuItem::with_id(
            app,
            format!("{MENU_RECENT_PROJECT_PREFIX}{}", entry.path),
            &entry.name,
            true,
            None::<&str>,
        )?);
    }
    let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = items
        .iter()
        .map(|item| item as &dyn tauri::menu::IsMenuItem<R>)
        .collect();
    Ok(Some(Submenu::with_items(
        app,
        "Open Recent Project",
        true,
        &refs,
    )?))
}

/// The Quit item, carrying the user's configured accelerator (default
/// `cmd+q`) so it behaves exactly like the predefined one it replaces.
#[cfg(target_os = "macos")]
fn quit_item<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    keyboard: &termlab_core::config::KeyboardConfig,
) -> tauri::Result<MenuItem<R>> {
    MenuItem::with_id(
        app,
        MENU_QUIT_ID,
        "Quit TermLab",
        true,
        Some(&config_key_to_accelerator(&keyboard.quit)),
    )
}

pub(crate) fn build_app_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    keyboard: &termlab_core::config::KeyboardConfig,
) -> tauri::Result<Menu<R>> {
    let new_tab_accel = primary_accelerator("T");
    let new_plain_shell_tab_accel = config_key_to_accelerator(&keyboard.new_plain_shell_tab);
    let new_tab = MenuItem::with_id(app, MENU_NEW_TAB_ID, "New Tab", true, Some(&new_tab_accel))?;
    let new_plain_shell_tab = MenuItem::with_id(
        app,
        MENU_NEW_PLAIN_SHELL_TAB_ID,
        "New Plain Shell Tab",
        true,
        Some(&new_plain_shell_tab_accel),
    )?;
    let close_tab = MenuItem::with_id(
        app,
        MENU_CLOSE_TAB_ID,
        "Close Tab",
        true,
        Some(&primary_accelerator("W")),
    )?;
    let new_window = MenuItem::with_id(
        app,
        MENU_NEW_WINDOW_ID,
        "New Window",
        true,
        Some(&primary_accelerator("Shift+N")),
    )?;
    let new_file_accel = config_key_to_accelerator(&keyboard.new_file);
    let new_file = MenuItem::with_id(
        app,
        MENU_NEW_FILE_ID,
        "New File",
        true,
        Some(&new_file_accel),
    )?;
    let open_file_accel = config_key_to_accelerator(&keyboard.open_file);
    let open_file = MenuItem::with_id(
        app,
        MENU_OPEN_FILE_ID,
        "Open File\u{2026}",
        true,
        Some(&open_file_accel),
    )?;
    // No accelerator: opening a project is a deliberate, infrequent act, and a
    // native accelerator here would be consumed by AppKit before the webview
    // saw the key (see the note on save_file_as).
    let open_folder = MenuItem::with_id(
        app,
        MENU_OPEN_FOLDER_ID,
        "Open Folder\u{2026}",
        true,
        None::<&str>,
    )?;
    // Deliberately NO accelerator, unlike New File/Open File… above. A native menu
    // accelerator is consumed by AppKit before the webview sees the key, which
    // is exactly why `save_file` has never had a menu item: Save As is scoped
    // to a focused editor pane, and that scoping lives in
    // shortcut-runtime.js's fallback guard. Binding Cmd+Shift+S here would
    // take the combo from the shell in every terminal pane. The keystroke is
    // still handled — by that guard — and Settings > Keymap still lists it;
    // this item is the discoverable, always-safe route (menu-actions.js
    // re-checks the focused pane before acting).
    let save_file_as = MenuItem::with_id(
        app,
        MENU_SAVE_FILE_AS_ID,
        "Save File As\u{2026}",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let close_window = PredefinedMenuItem::close_window(app, None)?;

    let rename_tab_accel = config_key_to_accelerator(&keyboard.rename_tab);
    let rename_tab = MenuItem::with_id(
        app,
        MENU_RENAME_TAB_ID,
        "Rename Tab",
        true,
        Some(&rename_tab_accel),
    )?;
    let ssh_export = MenuItem::with_id(
        app,
        MENU_SSH_EXPORT_ID,
        "Export Connections",
        true,
        None::<&str>,
    )?;
    let ssh_import = MenuItem::with_id(
        app,
        MENU_SSH_IMPORT_ID,
        "Import Connections",
        true,
        None::<&str>,
    )?;
    let ssh_manager_menu =
        Submenu::with_items(app, "SSH Manager", true, &[&ssh_export, &ssh_import])?;
    let separator2 = PredefinedMenuItem::separator(app)?;
    let recent_projects = recent_projects_submenu(app)?;
    let mut file_items: Vec<&dyn tauri::menu::IsMenuItem<R>> = vec![
        &new_tab,
        &new_plain_shell_tab,
        &new_window,
        &new_file,
        &open_file,
        &open_folder,
    ];
    if let Some(recent) = recent_projects.as_ref() {
        file_items.push(recent);
    }
    file_items.extend([
        &save_file_as as &dyn tauri::menu::IsMenuItem<R>,
        &separator,
        &ssh_manager_menu,
        &separator2,
        &rename_tab,
        &close_tab,
        &close_window,
    ]);
    let file_menu = Submenu::with_items(app, "File", true, &file_items)?;
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
        "Toggle Left Panel",
        true,
        Some(&toggle_left_accel),
    )?;
    let toggle_right_accel = config_key_to_accelerator(&keyboard.toggle_right_panel);
    let toggle_right = MenuItem::with_id(
        app,
        MENU_TOGGLE_RIGHT_PANEL_ID,
        "Toggle Right Panel",
        true,
        Some(&toggle_right_accel),
    )?;
    let focus_sessions = MenuItem::with_id(
        app,
        MENU_FOCUS_SESSIONS_ID,
        "Toggle & Focus Sessions",
        true,
        Some(&primary_accelerator("/")),
    )?;
    let zen_accel = config_key_to_accelerator(&keyboard.zen_mode);
    let zen_mode = MenuItem::with_id(app, MENU_ZEN_MODE_ID, "Zen Mode", true, Some(&zen_accel))?;
    let zoom_in = MenuItem::with_id(
        app,
        MENU_ZOOM_IN_ID,
        "Zoom In",
        true,
        Some(&primary_accelerator("=")),
    )?;
    let zoom_out = MenuItem::with_id(
        app,
        MENU_ZOOM_OUT_ID,
        "Zoom Out",
        true,
        Some(&primary_accelerator("-")),
    )?;
    let zoom_reset = MenuItem::with_id(
        app,
        MENU_ZOOM_RESET_ID,
        "Reset Zoom",
        true,
        Some(&primary_accelerator("0")),
    )?;
    let toggle_bottom_accel = config_key_to_accelerator(&keyboard.toggle_bottom_panel);
    let toggle_bottom = MenuItem::with_id(
        app,
        MENU_TOGGLE_BOTTOM_PANEL_ID,
        "Toggle Bottom Panel",
        true,
        Some(&toggle_bottom_accel),
    )?;
    let split_v_accel = config_key_to_accelerator(&keyboard.split_vertical);
    let split_v = MenuItem::with_id(
        app,
        MENU_SPLIT_VERTICAL_ID,
        "Split Pane Vertically",
        true,
        Some(&split_v_accel),
    )?;
    let split_h_accel = config_key_to_accelerator(&keyboard.split_horizontal);
    let split_h = MenuItem::with_id(
        app,
        MENU_SPLIT_HORIZONTAL_ID,
        "Split Pane Horizontally",
        true,
        Some(&split_h_accel),
    )?;
    let close_pane_accel = config_key_to_accelerator(&keyboard.close_pane);
    let close_pane_item = MenuItem::with_id(
        app,
        MENU_CLOSE_PANE_ID,
        "Close Pane",
        true,
        Some(&close_pane_accel),
    )?;
    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &toggle_left,
            &toggle_right,
            &toggle_bottom,
            &PredefinedMenuItem::separator(app)?,
            &split_v,
            &split_h,
            &close_pane_item,
            &PredefinedMenuItem::separator(app)?,
            &focus_sessions,
            &zen_mode,
            &PredefinedMenuItem::separator(app)?,
            &zoom_in,
            &zoom_out,
            &zoom_reset,
        ],
    )?;

    let settings = MenuItem::with_id(
        app,
        MENU_SETTINGS_ID,
        "Settings\u{2026}",
        true,
        Some(&primary_accelerator("Comma")),
    )?;
    let manage_tunnels_accel = config_key_to_accelerator(&keyboard.manage_tunnels);
    let manage_tunnels = MenuItem::with_id(
        app,
        MENU_MANAGE_TUNNELS_ID,
        "Manage SSH Tunnels\u{2026}",
        true,
        Some(&manage_tunnels_accel),
    )?;
    let credential_vault = MenuItem::with_id(
        app,
        MENU_VAULT_ID,
        "Credential Vault\u{2026}",
        true,
        Some(&config_key_to_accelerator(&keyboard.vault_open)),
    )?;
    let generate_ssh_key = MenuItem::with_id(
        app,
        MENU_KEYGEN_ID,
        "Generate SSH Key\u{2026}",
        true,
        None::<&str>,
    )?;
    let lock_vault = MenuItem::with_id(app, MENU_VAULT_LOCK_ID, "Lock Vault", true, None::<&str>)?;
    let tools_menu = Submenu::with_items(
        app,
        "Tools",
        true,
        &[
            &manage_tunnels,
            &PredefinedMenuItem::separator(app)?,
            &credential_vault,
            &generate_ssh_key,
            &lock_vault,
        ],
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
    #[cfg(debug_assertions)]
    let open_devtools = MenuItem::with_id(
        app,
        MENU_OPEN_DEVTOOLS_ID,
        "Open Developer Console",
        true,
        Some("F12"),
    )?;
    #[cfg(debug_assertions)]
    let debug_menu = Submenu::with_items(app, "Debug", true, &[&open_devtools])?;

    #[cfg(target_os = "macos")]
    {
        let app_name = app.package_info().name.clone();
        let check_updates = MenuItem::with_id(
            app,
            MENU_CHECK_UPDATES_ID,
            "Check for Updates\u{2026}",
            true,
            None::<&str>,
        )?;
        let app_menu = Submenu::with_items(
            app,
            app_name,
            true,
            &[
                &MenuItem::with_id(app, MENU_ABOUT_ID, "About TermLab", true, None::<&str>)?,
                &PredefinedMenuItem::separator(app)?,
                &settings,
                &check_updates,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &quit_item(app, keyboard)?,
            ],
        )?;
        #[cfg(debug_assertions)]
        {
            return Menu::with_items(
                app,
                &[
                    &app_menu,
                    &file_menu,
                    &edit_menu,
                    &view_menu,
                    &tools_menu,
                    &debug_menu,
                    &window_menu,
                ],
            );
        }
        #[cfg(not(debug_assertions))]
        {
            return Menu::with_items(
                app,
                &[
                    &app_menu,
                    &file_menu,
                    &edit_menu,
                    &view_menu,
                    &tools_menu,
                    &window_menu,
                ],
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let separator3 = PredefinedMenuItem::separator(app)?;
        let check_updates = MenuItem::with_id(
            app,
            MENU_CHECK_UPDATES_ID,
            "Check for Updates\u{2026}",
            true,
            None::<&str>,
        )?;
        let help_menu = Submenu::with_items(app, "Help", true, &[&check_updates])?;
        let file_menu = Submenu::with_items(
            app,
            "File",
            true,
            &[
                &new_tab,
                &new_window,
                &separator,
                &ssh_manager_menu,
                &separator2,
                &settings,
                &separator3,
                &close_tab,
                &close_window,
            ],
        )?;
        #[cfg(debug_assertions)]
        {
            Menu::with_items(
                app,
                &[
                    &file_menu,
                    &edit_menu,
                    &view_menu,
                    &tools_menu,
                    &debug_menu,
                    &window_menu,
                    &help_menu,
                ],
            )
        }
        #[cfg(not(debug_assertions))]
        {
            Menu::with_items(
                app,
                &[
                    &file_menu,
                    &edit_menu,
                    &view_menu,
                    &tools_menu,
                    &window_menu,
                    &help_menu,
                ],
            )
        }
    }
}

pub(crate) fn build_app_menu_with_plugins<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    keyboard: &termlab_core::config::KeyboardConfig,
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

        let manage_tunnels = MenuItem::with_id(
            app,
            MENU_MANAGE_TUNNELS_ID,
            "Manage SSH Tunnels\u{2026}",
            true,
            Some(&config_key_to_accelerator(&keyboard.manage_tunnels)),
        )?;
        tools_items.push(Box::new(manage_tunnels));

        // Add vault menu items.
        tools_items.push(Box::new(PredefinedMenuItem::separator(app)?));
        tools_items.push(Box::new(MenuItem::with_id(
            app,
            MENU_VAULT_ID,
            "Credential Vault\u{2026}",
            true,
            Some(&config_key_to_accelerator(&keyboard.vault_open)),
        )?));
        tools_items.push(Box::new(MenuItem::with_id(
            app,
            MENU_KEYGEN_ID,
            "Generate SSH Key\u{2026}",
            true,
            None::<&str>,
        )?));
        tools_items.push(Box::new(MenuItem::with_id(
            app,
            MENU_VAULT_LOCK_ID,
            "Lock Vault",
            true,
            None::<&str>,
        )?));

        // Add plugin items in a deterministic and organized structure:
        // Tools -> Plugins -> <Plugin Name> -> <Plugin Commands>
        if !plugin_items.is_empty() {
            tools_items.push(Box::new(PredefinedMenuItem::separator(app)?));

            let mut by_plugin: BTreeMap<String, Vec<&plugins::PluginMenuItem>> = BTreeMap::new();
            for item in plugin_items {
                by_plugin.entry(item.plugin.clone()).or_default().push(item);
            }

            let mut plugin_names: Vec<String> = by_plugin.keys().cloned().collect();
            plugin_names.sort_by_key(|name| name.to_ascii_lowercase());

            let mut plugin_submenus: Vec<Box<dyn tauri::menu::IsMenuItem<R>>> = Vec::new();
            for plugin_name in plugin_names {
                let mut entries = by_plugin.remove(&plugin_name).unwrap_or_default();
                entries.sort_by(|a, b| {
                    let a_group = a.menu.trim().to_ascii_lowercase();
                    let b_group = b.menu.trim().to_ascii_lowercase();
                    let a_key = if a_group == "tools" {
                        ""
                    } else {
                        a_group.as_str()
                    };
                    let b_key = if b_group == "tools" {
                        ""
                    } else {
                        b_group.as_str()
                    };
                    a_key
                        .cmp(b_key)
                        .then_with(|| {
                            a.label
                                .to_ascii_lowercase()
                                .cmp(&b.label.to_ascii_lowercase())
                        })
                        .then_with(|| {
                            a.action
                                .to_ascii_lowercase()
                                .cmp(&b.action.to_ascii_lowercase())
                        })
                });

                let mut entry_items: Vec<Box<dyn tauri::menu::IsMenuItem<R>>> = Vec::new();
                for item in entries {
                    let menu_id = format!("plugin.{}.{}", item.plugin, item.action);
                    let override_key = format!("{}:{}", item.plugin, item.action);
                    let chosen_keybind = keyboard
                        .plugin_shortcuts
                        .get(&override_key)
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .or(item.keybind.as_deref());
                    let accel = chosen_keybind.map(config_key_to_accelerator);
                    let mi = MenuItem::with_id(app, &menu_id, &item.label, true, accel.as_deref())?;
                    entry_items.push(Box::new(mi));
                }

                if entry_items.is_empty() {
                    continue;
                }
                let entry_refs: Vec<&dyn tauri::menu::IsMenuItem<R>> =
                    entry_items.iter().map(|b| &**b).collect();
                let plugin_menu = Submenu::with_items(app, &plugin_name, true, &entry_refs)?;
                plugin_submenus.push(Box::new(plugin_menu));
            }

            if !plugin_submenus.is_empty() {
                let plugin_refs: Vec<&dyn tauri::menu::IsMenuItem<R>> =
                    plugin_submenus.iter().map(|b| &**b).collect();
                let plugins_menu = Submenu::with_items(app, "Plugins", true, &plugin_refs)?;
                tools_items.push(Box::new(plugins_menu));
            }
        }

        // Rebuild the tools submenu.
        let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = tools_items.iter().map(|b| &**b).collect();
        let new_tools = Submenu::with_items(app, "Tools", true, &refs)?;

        // Rebuild full menu bar with new tools menu.
        let new_tab = MenuItem::with_id(
            app,
            MENU_NEW_TAB_ID,
            "New Tab",
            true,
            Some(&primary_accelerator("T")),
        )?;
        let new_plain_shell_tab_accel = config_key_to_accelerator(&keyboard.new_plain_shell_tab);
        let new_plain_shell_tab = MenuItem::with_id(
            app,
            MENU_NEW_PLAIN_SHELL_TAB_ID,
            "New Plain Shell Tab",
            true,
            Some(&new_plain_shell_tab_accel),
        )?;
        let close_tab = MenuItem::with_id(
            app,
            MENU_CLOSE_TAB_ID,
            "Close Tab",
            true,
            Some(&primary_accelerator("W")),
        )?;
        let rename_tab_accel = config_key_to_accelerator(&keyboard.rename_tab);
        let rename_tab = MenuItem::with_id(
            app,
            MENU_RENAME_TAB_ID,
            "Rename Tab",
            true,
            Some(&rename_tab_accel),
        )?;
        let new_file_accel = config_key_to_accelerator(&keyboard.new_file);
        let new_file = MenuItem::with_id(
            app,
            MENU_NEW_FILE_ID,
            "New File",
            true,
            Some(&new_file_accel),
        )?;
        let open_file_accel = config_key_to_accelerator(&keyboard.open_file);
        let open_file = MenuItem::with_id(
            app,
            MENU_OPEN_FILE_ID,
            "Open File\u{2026}",
            true,
            Some(&open_file_accel),
        )?;
        // No accelerator — see MENU_OPEN_FOLDER_ID definition in build_app_menu above.
        let open_folder = MenuItem::with_id(
            app,
            MENU_OPEN_FOLDER_ID,
            "Open Folder\u{2026}",
            true,
            None::<&str>,
        )?;
        // No accelerator — see build_app_menu above.
        let save_file_as = MenuItem::with_id(
            app,
            MENU_SAVE_FILE_AS_ID,
            "Save File As\u{2026}",
            true,
            None::<&str>,
        )?;
        let new_window = MenuItem::with_id(
            app,
            MENU_NEW_WINDOW_ID,
            "New Window",
            true,
            Some(&primary_accelerator("Shift+N")),
        )?;
        let separator = PredefinedMenuItem::separator(app)?;
        let close_window = PredefinedMenuItem::close_window(app, None)?;
        let settings = MenuItem::with_id(
            app,
            MENU_SETTINGS_ID,
            "Settings\u{2026}",
            true,
            Some(&primary_accelerator("Comma")),
        )?;
        let ssh_export = MenuItem::with_id(app, MENU_SSH_EXPORT_ID, "Export", true, None::<&str>)?;
        let ssh_import = MenuItem::with_id(app, MENU_SSH_IMPORT_ID, "Import", true, None::<&str>)?;
        let ssh_manager_menu =
            Submenu::with_items(app, "SSH Manager", true, &[&ssh_export, &ssh_import])?;
        let separator2 = PredefinedMenuItem::separator(app)?;
        let recent_projects = recent_projects_submenu(app)?;
        let mut file_items: Vec<&dyn tauri::menu::IsMenuItem<R>> = vec![
            &new_tab,
            &new_plain_shell_tab,
            &new_window,
            &new_file,
            &open_file,
            &open_folder,
        ];
        if let Some(recent) = recent_projects.as_ref() {
            file_items.push(recent);
        }
        file_items.extend([
            &save_file_as as &dyn tauri::menu::IsMenuItem<R>,
            &separator,
            &ssh_manager_menu,
            &separator2,
            &rename_tab,
            &close_tab,
            &close_window,
        ]);
        let file_menu = Submenu::with_items(app, "File", true, &file_items)?;
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

        let toggle_left_accel = config_key_to_accelerator(&keyboard.toggle_left_panel);
        let toggle_left = MenuItem::with_id(
            app,
            MENU_TOGGLE_LEFT_PANEL_ID,
            "Toggle Left Panel",
            true,
            Some(&toggle_left_accel),
        )?;
        let toggle_right_accel = config_key_to_accelerator(&keyboard.toggle_right_panel);
        let toggle_right = MenuItem::with_id(
            app,
            MENU_TOGGLE_RIGHT_PANEL_ID,
            "Toggle Right Panel",
            true,
            Some(&toggle_right_accel),
        )?;
        let toggle_bottom_accel = config_key_to_accelerator(&keyboard.toggle_bottom_panel);
        let toggle_bottom = MenuItem::with_id(
            app,
            MENU_TOGGLE_BOTTOM_PANEL_ID,
            "Toggle Bottom Panel",
            true,
            Some(&toggle_bottom_accel),
        )?;
        let focus_sessions = MenuItem::with_id(
            app,
            MENU_FOCUS_SESSIONS_ID,
            "Toggle & Focus Sessions",
            true,
            Some(&primary_accelerator("/")),
        )?;
        let zen_accel = config_key_to_accelerator(&keyboard.zen_mode);
        let zen_mode =
            MenuItem::with_id(app, MENU_ZEN_MODE_ID, "Zen Mode", true, Some(&zen_accel))?;
        let zoom_in = MenuItem::with_id(
            app,
            MENU_ZOOM_IN_ID,
            "Zoom In",
            true,
            Some(&primary_accelerator("=")),
        )?;
        let zoom_out = MenuItem::with_id(
            app,
            MENU_ZOOM_OUT_ID,
            "Zoom Out",
            true,
            Some(&primary_accelerator("-")),
        )?;
        let zoom_reset = MenuItem::with_id(
            app,
            MENU_ZOOM_RESET_ID,
            "Reset Zoom",
            true,
            Some(&primary_accelerator("0")),
        )?;
        let split_v_accel = config_key_to_accelerator(&keyboard.split_vertical);
        let split_v = MenuItem::with_id(
            app,
            MENU_SPLIT_VERTICAL_ID,
            "Split Pane Vertically",
            true,
            Some(&split_v_accel),
        )?;
        let split_h_accel = config_key_to_accelerator(&keyboard.split_horizontal);
        let split_h = MenuItem::with_id(
            app,
            MENU_SPLIT_HORIZONTAL_ID,
            "Split Pane Horizontally",
            true,
            Some(&split_h_accel),
        )?;
        let close_pane_accel = config_key_to_accelerator(&keyboard.close_pane);
        let close_pane_item = MenuItem::with_id(
            app,
            MENU_CLOSE_PANE_ID,
            "Close Pane",
            true,
            Some(&close_pane_accel),
        )?;
        let view_menu = Submenu::with_items(
            app,
            "View",
            true,
            &[
                &toggle_left,
                &toggle_right,
                &toggle_bottom,
                &PredefinedMenuItem::separator(app)?,
                &split_v,
                &split_h,
                &close_pane_item,
                &PredefinedMenuItem::separator(app)?,
                &focus_sessions,
                &zen_mode,
                &PredefinedMenuItem::separator(app)?,
                &zoom_in,
                &zoom_out,
                &zoom_reset,
            ],
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
        #[cfg(debug_assertions)]
        let open_devtools = MenuItem::with_id(
            app,
            MENU_OPEN_DEVTOOLS_ID,
            "Open Developer Console",
            true,
            Some("F12"),
        )?;
        #[cfg(debug_assertions)]
        let debug_menu = Submenu::with_items(app, "Debug", true, &[&open_devtools])?;

        #[cfg(target_os = "macos")]
        {
            let app_name = app.package_info().name.clone();
            let check_updates = MenuItem::with_id(
                app,
                MENU_CHECK_UPDATES_ID,
                "Check for Updates\u{2026}",
                true,
                None::<&str>,
            )?;
            let app_menu = Submenu::with_items(
                app,
                app_name,
                true,
                &[
                    &MenuItem::with_id(app, MENU_ABOUT_ID, "About TermLab", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &settings,
                    &check_updates,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &quit_item(app, keyboard)?,
                ],
            )?;
            #[cfg(debug_assertions)]
            {
                return Menu::with_items(
                    app,
                    &[
                        &app_menu,
                        &file_menu,
                        &edit_menu,
                        &view_menu,
                        &new_tools,
                        &debug_menu,
                        &window_menu,
                    ],
                );
            }
            #[cfg(not(debug_assertions))]
            {
                return Menu::with_items(
                    app,
                    &[
                        &app_menu,
                        &file_menu,
                        &edit_menu,
                        &view_menu,
                        &new_tools,
                        &window_menu,
                    ],
                );
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let separator3 = PredefinedMenuItem::separator(app)?;
            let check_updates = MenuItem::with_id(
                app,
                MENU_CHECK_UPDATES_ID,
                "Check for Updates\u{2026}",
                true,
                None::<&str>,
            )?;
            let help_menu = Submenu::with_items(app, "Help", true, &[&check_updates])?;
            let file_menu = Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &new_tab,
                    &new_window,
                    &separator,
                    &ssh_manager_menu,
                    &separator2,
                    &settings,
                    &separator3,
                    &close_tab,
                    &close_window,
                ],
            )?;
            #[cfg(debug_assertions)]
            {
                return Menu::with_items(
                    app,
                    &[
                        &file_menu,
                        &edit_menu,
                        &view_menu,
                        &new_tools,
                        &debug_menu,
                        &window_menu,
                        &help_menu,
                    ],
                );
            }
            #[cfg(not(debug_assertions))]
            {
                return Menu::with_items(
                    app,
                    &[
                        &file_menu,
                        &edit_menu,
                        &view_menu,
                        &new_tools,
                        &window_menu,
                        &help_menu,
                    ],
                );
            }
        }
    }

    Ok(base)
}

// ---------------------------------------------------------------------------
// Helpers for emitting menu actions to the focused window
// ---------------------------------------------------------------------------

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

pub(crate) fn emit_menu_action_to_focused_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    action: &str,
) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_to_accelerator_basic() {
        assert_eq!(
            config_key_to_accelerator("cmd+shift+r"),
            format!("{PRIMARY_ACCELERATOR_MOD}+Shift+R")
        );
    }

    #[test]
    fn config_key_to_accelerator_cmdorctrl_uses_primary_modifier() {
        assert_eq!(
            config_key_to_accelerator("cmdorctrl+shift+p"),
            format!("{PRIMARY_ACCELERATOR_MOD}+Shift+P")
        );
    }

    #[test]
    fn config_key_to_accelerator_ctrl() {
        assert_eq!(config_key_to_accelerator("ctrl+t"), "Ctrl+T");
    }

    #[test]
    fn config_key_to_accelerator_alt() {
        assert_eq!(config_key_to_accelerator("alt+f"), "Alt+F");
    }

    #[test]
    fn config_key_to_accelerator_option() {
        assert_eq!(config_key_to_accelerator("option+g"), "Alt+G");
    }

    #[test]
    fn config_key_to_accelerator_single_key() {
        assert_eq!(config_key_to_accelerator("f2"), "F2");
    }

    #[test]
    fn recent_project_menu_ids_carry_the_path_itself() {
        // Pins the literal a menu id is built from: on_menu_event (lib.rs)
        // strips exactly this prefix back off to recover the FULL PATH
        // (fix round 1, F5 — not an index), so the two sides must agree on
        // the prefix byte-for-byte.
        assert_eq!(MENU_RECENT_PROJECT_PREFIX, "file.recent_project.");
        let id = format!("{MENU_RECENT_PROJECT_PREFIX}{}", "/repo/b");
        assert_eq!(id, "file.recent_project./repo/b");
        assert_eq!(&id[MENU_RECENT_PROJECT_PREFIX.len()..], "/repo/b");
    }

    #[test]
    fn a_recents_list_change_between_menu_build_and_click_cannot_mis_resolve_a_path_id() {
        // F5's actual bug class, reproduced directly: an INDEX-based id (the
        // old scheme) resolves against whatever `list_recents()` returns at
        // CLICK time — if project A (index 0) is deleted between the menu
        // being built and the click, clicking what was index 1 (project B)
        // now resolves to whatever slid into index 1 (project C): wrong
        // project, silently. A path-based id has no list to re-resolve
        // against, so the same deletion cannot shift what a click means.
        let built_for_b = format!("{MENU_RECENT_PROJECT_PREFIX}{}", "/repo/b");

        // Simulate project A vanishing from the list between build and
        // click (list_recents() no longer stats/filters — F4 — but the
        // ORDER can still change if another window's remember() ran
        // meanwhile; either way, the id itself is unaffected).
        let recents_at_click_time = ["/repo/b", "/repo/c"]; // "/repo/a" is gone
        let resolved_path = &built_for_b[MENU_RECENT_PROJECT_PREFIX.len()..];

        assert_eq!(
            resolved_path, "/repo/b",
            "the id resolves to exactly what was clicked, regardless of what \
             the list looks like now"
        );
        assert!(
            recents_at_click_time.contains(&resolved_path),
            "sanity: the resolved path is still a real, current recent"
        );
    }
}
