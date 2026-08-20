//! General Tauri commands: config queries, layout persistence, zoom, and
//! menu rebuilding.
//!
//! These are the "miscellaneous" commands that don't belong in a more specific
//! module like `pty` or `remote`.

use std::sync::Arc;

use termlab_core::config;
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
        "zen_mode_shortcut": cfg.termlab.keyboard.zen_mode,
        "decorations": dec,
        "new_window_zen_mode": cfg.window.new_window_zen_mode,
        // Default window size in terminal cells. Rust opens the window at an
        // estimate; the frontend corrects it once it can measure a real cell.
        "window_columns": cfg.window.dimensions.columns,
        "window_lines": cfg.window.dimensions.lines,
        "platform": std::env::consts::OS,
        "debug_build": cfg!(debug_assertions),
        "notification_position": cfg.termlab.ui.notification_position,
        "native_notifications": cfg.termlab.ui.native_notifications,
        "disable_animations": cfg.termlab.ui.disable_animations,
        "ui_font_family": cfg.termlab.ui.font_family,
        "ui_font_size": cfg.termlab.ui.font_size,
        // The editor's vim keymap. Carried here rather than on
        // get_terminal_config because it is not a terminal setting, and this
        // payload is already re-fetched on every `config-changed` — which is
        // what lets the toggle reach open editor panes without a restart.
        "editor_vim_mode": cfg.editor.vim_mode,
        "ui_font_small": cfg.termlab.ui.font.small,
        "ui_font_list": cfg.termlab.ui.font.list,
        "ui_font_normal": cfg.termlab.ui.font.normal,
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

/// Return the user's home directory, used as the app's "workspace" label.
///
/// This is intentionally static: captured once at startup via
/// `dirs::home_dir()` (`TauriState::workspace_dir`) — not the process's
/// actual working directory, and not read from any PTY's shell — and never
/// re-read afterward, so it keeps matching the reference app's project-name
/// window-title semantics (a fixed label for the window) rather than
/// drifting if a terminal pane later `cd`s elsewhere. Frontend callers
/// should treat `None` as "no workspace known" and fall back to another
/// source (e.g. a pane's live cwd) rather than treating it as an error.
#[tauri::command]
pub(crate) fn get_workspace_dir(state: tauri::State<'_, TauriState>) -> Option<String> {
    state.workspace_dir.clone()
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

/// Resolve the terminal palette for this window.
///
/// `resolved_appearance` ('dark' | 'light') is OPTIONAL: the frontend passes
/// `termlabAppearance.current()`, which is the only place the `system`
/// appearance is actually resolved (it lives in `matchMedia`, invisible to
/// Rust). An invoke that omits it — any caller predating this argument —
/// resolves as dark, which is the pre-existing behavior for every theme name
/// except the new reserved `auto`.
#[tauri::command]
pub(crate) fn get_theme_colors(
    state: tauri::State<'_, TauriState>,
    resolved_appearance: Option<String>,
) -> theme::ThemeColors {
    let cfg = state.config.read();
    theme::resolve_theme_colors_for_appearance(&cfg, resolved_appearance.as_deref())
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
    new_plain_shell_tab: String,
    toggle_right_panel: String,
    toggle_left_panel: String,
    toggle_bottom_panel: String,
    split_vertical: String,
    split_horizontal: String,
    close_pane: String,
    rename_tab: String,
    manage_tunnels: String,
    vault_open: String,
    /// Editor bindings, for the custom titlebar's File menu.
    ///
    /// `save_file_as` is display only — it carries `noAccel`, because it is
    /// scoped to a focused editor pane and that scoping lives in
    /// shortcut-runtime.js's guarded fallback table.
    ///
    /// `new_file` and `open_file` are display AND accelerator: the titlebar
    /// registers them at router priority 115. They must therefore be READ from
    /// here rather than hardcoded — a hardcoded default outranks the
    /// configurable binding at 75/80, so a user who rebinds the action keeps a
    /// hard-bound default they cannot free.
    new_file: String,
    open_file: String,
    save_file_as: String,
    /// Same display-AND-accelerator contract as `new_file` above: these four
    /// are configurable in `[termlab.keyboard]` and live titlebar bindings,
    /// so the frontend must read them from this payload.
    new_tab: String,
    new_window: String,
    close_tab: String,
    settings: String,
}

#[tauri::command]
pub(crate) fn get_keyboard_shortcuts(state: tauri::State<'_, TauriState>) -> KeyboardShortcuts {
    let cfg = state.config.read();
    let kb = &cfg.termlab.keyboard;
    KeyboardShortcuts {
        new_plain_shell_tab: kb.new_plain_shell_tab.clone(),
        toggle_right_panel: kb.toggle_right_panel.clone(),
        toggle_left_panel: kb.toggle_left_panel.clone(),
        toggle_bottom_panel: kb.toggle_bottom_panel.clone(),
        split_vertical: kb.split_vertical.clone(),
        split_horizontal: kb.split_horizontal.clone(),
        close_pane: kb.close_pane.clone(),
        rename_tab: kb.rename_tab.clone(),
        manage_tunnels: kb.manage_tunnels.clone(),
        vault_open: kb.vault_open.clone(),
        new_file: kb.new_file.clone(),
        open_file: kb.open_file.clone(),
        save_file_as: kb.save_file_as.clone(),
        new_tab: kb.new_tab.clone(),
        new_window: kb.new_window.clone(),
        close_tab: kb.close_tab.clone(),
        settings: kb.settings.clone(),
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
    bottom_panel_height: Option<f64>,
    zen_mode: Option<bool>,
    tool_window_zones: Option<std::collections::HashMap<String, String>>,
    active_tool_windows: Option<std::collections::HashMap<String, String>>,
    tool_window_view_modes: Option<std::collections::HashMap<String, String>>,
    split_ratios: Option<SplitRatios>,
}

#[derive(Deserialize)]
pub(crate) struct SplitRatios {
    left: Option<f64>,
    right: Option<f64>,
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
    bottom_panel_height: f64,
    zen_mode: bool,
    tool_window_zones: std::collections::HashMap<String, String>,
    active_tool_windows: std::collections::HashMap<String, String>,
    /// The read-back twin of [`WindowLayout::tool_window_view_modes`], mirroring
    /// `tool_window_zones` above exactly. Without it the frontend could write a
    /// view mode but never learn it again, so a popped-out tool window would
    /// silently come back docked on the next launch.
    tool_window_view_modes: std::collections::HashMap<String, String>,
    left_split_ratio: f64,
    right_split_ratio: f64,
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

/// Project a persisted [`config::LayoutConfig`] onto the [`SavedLayout`] the
/// frontend loads. Pulled out as a pure function (no config I/O) for the same
/// reason [`merge_window_layout`] below was: the save and load sides of every
/// persisted layout field are then unit-testable as a matched pair.
fn saved_layout_from_state(layout: &config::LayoutConfig) -> SavedLayout {
    SavedLayout {
        window_width: layout.window_width as f64,
        window_height: layout.window_height as f64,
        ssh_panel_width: layout.right_panel_width as f64,
        ssh_panel_visible: layout.right_panel_visible,
        files_panel_width: layout.left_panel_width as f64,
        files_panel_visible: layout.left_panel_visible,
        bottom_panel_visible: layout.bottom_panel_visible,
        bottom_panel_height: layout.bottom_panel_height as f64,
        zen_mode: layout.zen_mode,
        tool_window_zones: layout.tool_window_zones.clone(),
        active_tool_windows: layout.active_tool_windows.clone(),
        tool_window_view_modes: layout.tool_window_view_modes.clone(),
        left_split_ratio: layout.left_split_ratio as f64,
        right_split_ratio: layout.right_split_ratio as f64,
    }
}

#[tauri::command]
pub(crate) fn get_saved_layout() -> SavedLayout {
    let state = config::load_persistent_state().unwrap_or_default();
    saved_layout_from_state(&state.layout)
}

/// Merge a frontend-sent [`WindowLayout`] into `state` — every field is
/// `Option`-gated and merges only when `Some`; an absent field leaves
/// whatever is already persisted untouched. Pulled out as a pure function
/// (no window handle, no config I/O) so this merge-only-when-Some contract
/// is unit-testable directly, the same way the panel-host / chooser
/// registries' pure logic is.
fn merge_window_layout(state: &mut config::LayoutConfig, layout: WindowLayout) {
    if let Some(w) = layout.ssh_panel_width {
        state.right_panel_width = w as f32;
    }
    if let Some(v) = layout.ssh_panel_visible {
        state.right_panel_visible = v;
    }
    if let Some(w) = layout.files_panel_width {
        state.left_panel_width = w as f32;
    }
    if let Some(v) = layout.files_panel_visible {
        state.left_panel_visible = v;
    }
    if let Some(v) = layout.bottom_panel_visible {
        state.bottom_panel_visible = v;
    }
    if let Some(h) = layout.bottom_panel_height {
        state.bottom_panel_height = h as f32;
    }
    if let Some(v) = layout.zen_mode {
        state.zen_mode = v;
    }
    if let Some(zones) = layout.tool_window_zones {
        state.tool_window_zones = zones;
    }
    if let Some(active_windows) = layout.active_tool_windows {
        state.active_tool_windows = active_windows;
    }
    if let Some(view_modes) = layout.tool_window_view_modes {
        state.tool_window_view_modes = view_modes;
    }
    if let Some(ratios) = layout.split_ratios {
        if let Some(l) = ratios.left {
            state.left_split_ratio = l as f32;
        }
        if let Some(r) = ratios.right {
            state.right_split_ratio = r as f32;
        }
    }
}

#[tauri::command]
pub(crate) fn save_window_layout(window: tauri::WebviewWindow, layout: WindowLayout) {
    let size = window.inner_size().unwrap_or_default();
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical_w = size.width as f64 / scale;
    let logical_h = size.height as f64 / scale;

    let mut state = config::load_persistent_state().unwrap_or_default();
    // Recorded for diagnostics only. Nothing reads these back for sizing any
    // more: windows open at the configured columns x lines (see
    // estimate_window_px and app/core/window-size.js), so restoring the last
    // session's pixels would defeat that setting — and made a window spawned
    // from a full-screen one open full-screen.
    state.layout.window_width = logical_w as f32;
    state.layout.window_height = logical_h as f32;
    merge_window_layout(&mut state.layout, layout);
    let _ = config::save_persistent_state(&state);
}

// ---------------------------------------------------------------------------
// Zoom
// ---------------------------------------------------------------------------

/// Persist the measured cell size and window chrome, so the NEXT launch can
/// open at the exact pixels for the configured columns x lines instead of an
/// estimate. Called by the frontend whenever its size correction converges;
/// rejected values are dropped rather than saved, because a garbage
/// measurement here means every future launch opens at a garbage size.
#[tauri::command]
pub(crate) fn save_window_metrics(
    state: tauri::State<'_, TauriState>,
    cell_width: f64,
    cell_height: f64,
    chrome_width: f64,
    chrome_height: f64,
) {
    // The fingerprint is stamped here from the SAME config the window was
    // sized from, rather than trusted from the frontend — a mislabelled
    // measurement would be applied to the wrong font forever.
    let (family, size) = {
        let cfg = state.config.read();
        let font = cfg.resolved_terminal_font();
        (font.normal.family.clone(), font.size)
    };
    let zoom = config::load_persistent_state()
        .unwrap_or_default()
        .layout
        .zoom_factor;
    let metrics = termlab_core::config::WindowMetrics {
        cell_width: cell_width as f32,
        cell_height: cell_height as f32,
        chrome_width: chrome_width as f32,
        chrome_height: chrome_height as f32,
        font_family: family,
        font_size: size,
        zoom: if zoom > 0.0 { zoom } else { 1.0 },
    };
    if !metrics.is_usable() {
        return;
    }
    let mut state = config::load_persistent_state().unwrap_or_default();
    if state.window_metrics == metrics {
        return; // steady state — no write on every launch
    }
    state.window_metrics = metrics;
    let _ = config::save_persistent_state(&state);
}

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
        .map(|c| c.termlab.keyboard)
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

#[cfg(test)]
mod window_layout_merge_tests {
    use super::*;

    /// A payload with every field absent — the "the frontend didn't send
    /// this" case every merge-only-when-Some assertion below starts from.
    fn empty_layout() -> WindowLayout {
        WindowLayout {
            ssh_panel_width: None,
            ssh_panel_visible: None,
            files_panel_width: None,
            files_panel_visible: None,
            bottom_panel_visible: None,
            bottom_panel_height: None,
            zen_mode: None,
            tool_window_zones: None,
            active_tool_windows: None,
            tool_window_view_modes: None,
            split_ratios: None,
        }
    }

    #[test]
    fn a_present_view_modes_map_replaces_whatever_was_persisted() {
        let mut state = config::LayoutConfig::default();
        state
            .tool_window_view_modes
            .insert("stale-id".to_string(), "window".to_string());

        let mut incoming = std::collections::HashMap::new();
        incoming.insert("ssh-sessions".to_string(), "dock".to_string());
        let layout = WindowLayout {
            tool_window_view_modes: Some(incoming.clone()),
            ..empty_layout()
        };

        merge_window_layout(&mut state, layout);

        assert_eq!(state.tool_window_view_modes, incoming);
        assert!(
            !state.tool_window_view_modes.contains_key("stale-id"),
            "a present map REPLACES the old one outright, it does not merge \
             key-by-key into it"
        );
    }

    #[test]
    fn an_absent_view_modes_field_leaves_the_persisted_map_untouched() {
        let mut state = config::LayoutConfig::default();
        state
            .tool_window_view_modes
            .insert("ssh-sessions".to_string(), "window".to_string());
        let before = state.tool_window_view_modes.clone();

        merge_window_layout(&mut state, empty_layout());

        assert_eq!(state.tool_window_view_modes, before);
    }

    /// The read-back mirror, asserted the same way the save side above is:
    /// what `save_window_layout` merged in must come back out of
    /// `get_saved_layout`. Mirrors `tool_window_zones`, checked alongside it
    /// so the two can never drift apart unnoticed.
    #[test]
    fn view_modes_survive_the_round_trip_back_to_the_frontend() {
        let mut state = config::LayoutConfig::default();

        let mut incoming = std::collections::HashMap::new();
        incoming.insert("ssh-sessions".to_string(), "window".to_string());
        incoming.insert("tunnels".to_string(), "dock".to_string());
        let mut zones = std::collections::HashMap::new();
        zones.insert("ssh-sessions".to_string(), "right-top".to_string());
        let layout = WindowLayout {
            tool_window_zones: Some(zones.clone()),
            tool_window_view_modes: Some(incoming.clone()),
            ..empty_layout()
        };
        merge_window_layout(&mut state, layout);

        let saved = saved_layout_from_state(&state);
        assert_eq!(
            saved.tool_window_view_modes, incoming,
            "a view mode the frontend saved must be readable again on the next \
             load, or a popped-out tool window comes back docked"
        );
        assert_eq!(saved.tool_window_zones, zones);
    }

    #[test]
    fn a_state_that_never_recorded_view_modes_reads_back_as_an_empty_map() {
        // Every state.toml written before this field existed.
        let saved = saved_layout_from_state(&config::LayoutConfig::default());
        assert!(saved.tool_window_view_modes.is_empty());
    }

    #[test]
    fn merge_only_when_some_holds_for_every_optional_field_at_once() {
        // A broader smoke test: a payload with everything absent except one
        // field (here, zen_mode) changes ONLY that field.
        let mut state = config::LayoutConfig::default();
        state
            .tool_window_zones
            .insert("ssh-sessions".to_string(), "right-top".to_string());
        let zones_before = state.tool_window_zones.clone();
        let width_before = state.right_panel_width;

        let layout = WindowLayout {
            zen_mode: Some(true),
            ..empty_layout()
        };
        merge_window_layout(&mut state, layout);

        assert!(state.zen_mode);
        assert_eq!(state.tool_window_zones, zones_before);
        assert_eq!(state.right_panel_width, width_before);
    }
}
