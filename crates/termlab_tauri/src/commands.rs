//! General Tauri commands: config queries, layout persistence, zoom, and
//! menu rebuilding.
//!
//! These are the "miscellaneous" commands that don't belong in a more specific
//! module like `pty` or `remote`.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use termlab_core::config;
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
        // Whether completion opens on its own as you type. Carried on the
        // same payload and for the same reason as the vim flag: it is
        // re-fetched on every `config-changed`, so the toggle reaches open
        // editor panes without a restart. Manual completion is unaffected
        // by it.
        "editor_lsp_suggestions_while_typing": cfg.editor.lsp.suggestions_while_typing,
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
    bottom: Option<f64>,
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
    bottom_split_ratio: f64,
    /// True exactly when this window resolved to a project AND that project
    /// already had its own `project_layouts` entry — i.e. it has been opened
    /// (and its layout saved) at least once before. Always `false` for a
    /// non-project window. The frontend's boot-reveal hand-off (Task 12/F1)
    /// uses this to tell a fresh project (always reveal Files, per spec §1)
    /// from a returning one (trust exactly what it saved, including a
    /// deliberately-closed panel or a different active tab).
    has_project_layout: bool,
}

#[tauri::command]
pub(crate) fn app_ready(window: tauri::WebviewWindow) {
    let _ = window.show();
    // Windows are built hidden and revealed here, which on macOS is not
    // enough to bring them forward: a window created by a *background* app
    // (the CLI/IPC "open this path" case — the user is in their terminal,
    // not in TermLab) shows up behind whatever they were looking at. Each
    // window is only ever ready once, so this focuses a genuinely new window
    // and is a no-op-ish nudge for the boot window, which is being presented
    // to the user at this exact moment anyway.
    let _ = window.set_focus();
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
fn saved_layout_from_state(layout: &config::LayoutConfig, has_project_layout: bool) -> SavedLayout {
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
        bottom_split_ratio: layout.bottom_split_ratio as f64,
        has_project_layout,
    }
}

/// The one seam every project-aware layout read OR write goes through to
/// decide "which `LayoutConfig` is this window's": `root` is the CALLING
/// window's project root (`None` for an ordinary window), never anything
/// else. Read and write are deliberately separate functions rather than one
/// `bool: for_write` — read never mutates `project_layouts` (an absent entry
/// falls back to the shared layout WITHOUT creating one), write always
/// resolves to an entry it can hand out `&mut` to. Extracted so the
/// isolation invariant (a project window's save can never reach the shared
/// entry, and vice versa) is a property of the TYPE SIGNATURE — a caller
/// physically cannot get both a `&LayoutConfig` and stuff it in the wrong
/// place — rather than a match arm a review has to re-verify by reading
/// (branch review F2: the previous shape was provably correct only by a
/// source grep, which passes just as well if the two arms are swapped).
fn layout_to_read<'a>(
    state: &'a config::PersistentState,
    root: Option<&str>,
) -> &'a config::LayoutConfig {
    match root {
        Some(root) => state.project_layouts.get(root).unwrap_or(&state.layout),
        None => &state.layout,
    }
}

/// The write-side twin of [`layout_to_read`]. A project root that has never
/// saved before gets a fresh entry seeded from the CURRENT shared layout
/// (the default project-window shape a brand new project boots with), so
/// the first save doesn't have to special-case "entry didn't exist yet".
fn layout_to_write<'a>(
    state: &'a mut config::PersistentState,
    root: Option<&str>,
) -> &'a mut config::LayoutConfig {
    match root {
        Some(root) => {
            let base = state.layout.clone();
            state
                .project_layouts
                .entry(root.to_string())
                .or_insert(base)
        }
        None => &mut state.layout,
    }
}

/// The layout this window should boot with. A project window gets its
/// PROJECT's saved layout; every other window gets the shared one. An absent
/// project entry falls back to the shared layout, which the frontend then
/// adjusts into the default project-window shape.
#[tauri::command]
pub(crate) fn get_saved_layout(
    window: tauri::WebviewWindow,
    projects: tauri::State<'_, parking_lot::Mutex<crate::project::ProjectRegistry>>,
) -> SavedLayout {
    let state = config::load_persistent_state().unwrap_or_default();
    let root = projects.lock().root_for(window.label());
    let has_project_layout = root
        .as_deref()
        .is_some_and(|r| state.project_layouts.contains_key(r));
    saved_layout_from_state(layout_to_read(&state, root.as_deref()), has_project_layout)
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
        if let Some(b) = ratios.bottom {
            state.bottom_split_ratio = b as f32;
        }
    }
}

#[tauri::command]
pub(crate) fn save_window_layout(window: tauri::WebviewWindow, layout: WindowLayout) {
    let size = window.inner_size().unwrap_or_default();
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical_w = size.width as f64 / scale;
    let logical_h = size.height as f64 / scale;

    let project_root = {
        let projects = window
            .app_handle()
            .state::<parking_lot::Mutex<crate::project::ProjectRegistry>>();
        projects.lock().root_for(window.label())
    };

    // Loaded, mutated, and saved as one locked span (config::
    // update_persistent_state) — this command runs on the main thread, but
    // the panel-host bounds debounce thread also writes state.toml now, so
    // an unlocked load-mutate-save here could interleave with it and drop
    // one side's mutation (branch review F2).
    let _ = config::update_persistent_state(|state| {
        // Recorded for diagnostics only (see the note above): nothing reads
        // these back for sizing.
        state.layout.window_width = logical_w as f32;
        state.layout.window_height = logical_h as f32;
        // A project window writes ONLY its project's entry. Otherwise a
        // project's panel arrangement would become the shape every ordinary
        // window opens in. Routed through `layout_to_write` (branch review
        // F2) rather than a match arm here — see its doc comment.
        merge_window_layout(layout_to_write(state, project_root.as_deref()), layout);
        true
    });
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
    // Reading zoom_factor and deciding "is this a no-op" both happen inside
    // the lock now, against the freshest load, instead of a pre-lock read
    // that a concurrent writer (the panel-host bounds debounce thread) could
    // make stale.
    let _ = config::update_persistent_state(|state| {
        let zoom = state.layout.zoom_factor;
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
            return false;
        }
        if state.window_metrics == metrics {
            return false; // steady state — no write on every launch
        }
        state.window_metrics = metrics;
        true
    });
}

#[tauri::command]
pub(crate) fn set_zoom_level(
    window: tauri::WebviewWindow,
    scale_factor: f64,
) -> Result<(), String> {
    window.set_zoom(scale_factor).map_err(|e| e.to_string())?;
    let _ = config::update_persistent_state(|state| {
        state.layout.zoom_factor = scale_factor as f32;
        true
    });
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

    /// A `bottom` ratio the frontend sends with `left`/`right` absent (the
    /// bottom-zone divider drag path never touches the sidebar splits) must
    /// merge into state on its own, and read back out of
    /// `saved_layout_from_state` unchanged — the same save/load pairing the
    /// view-modes tests above check for `tool_window_view_modes`.
    #[test]
    fn bottom_split_ratio_merges_and_reads_back_independently_of_left_and_right() {
        let mut state = config::LayoutConfig::default();

        let layout = WindowLayout {
            split_ratios: Some(SplitRatios {
                left: None,
                right: None,
                bottom: Some(0.25),
            }),
            ..empty_layout()
        };
        merge_window_layout(&mut state, layout);

        assert_eq!(state.bottom_split_ratio, 0.25);
        assert_eq!(
            state.left_split_ratio, 0.5,
            "an absent left ratio must not be disturbed by a bottom-only payload"
        );
        assert_eq!(state.right_split_ratio, 0.5);

        let saved = saved_layout_from_state(&state, false);
        assert_eq!(saved.bottom_split_ratio, 0.25);
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

        let saved = saved_layout_from_state(&state, false);
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
        let saved = saved_layout_from_state(&config::LayoutConfig::default(), false);
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

    /// F2 (fix round 1): the project/global isolation invariant, pinned by
    /// exercising `layout_to_read`/`layout_to_write` directly rather than by
    /// grepping `save_window_layout`'s source for a `match` shape. A source
    /// grep (what this invariant was checked by before this round) passes
    /// exactly as well if the two match arms are swapped — these tests
    /// would not. Nested here (rather than a sibling module) so it can
    /// reuse `empty_layout()` above.
    mod layout_isolation_tests {
        use super::*;

        fn state_with_global(zen_mode: bool, bottom_panel_height: f32) -> config::PersistentState {
            config::PersistentState {
                layout: config::LayoutConfig {
                    zen_mode,
                    bottom_panel_height,
                    ..config::LayoutConfig::default()
                },
                ..config::PersistentState::default()
            }
        }

        #[test]
        fn a_project_write_lands_only_in_that_projects_entry_the_global_layout_is_byte_identical() {
            let mut state = state_with_global(false, 100.0);
            let global_before = state.layout.clone();

            let layout = WindowLayout {
                zen_mode: Some(true),
                ..empty_layout()
            };
            merge_window_layout(layout_to_write(&mut state, Some("/repo")), layout);

            assert_eq!(
                state.layout, global_before,
                "a project window's write must not touch the shared layout at all"
            );
            assert_eq!(
                state.project_layouts.get("/repo").map(|l| l.zen_mode),
                Some(true),
                "the write must land in that project's own entry"
            );
            assert!(
                state.project_layouts.len() == 1,
                "only the ONE project written to gets an entry"
            );
        }

        #[test]
        fn a_non_project_write_leaves_project_layouts_untouched() {
            let mut state = state_with_global(false, 100.0);
            state
                .project_layouts
                .insert("/other-repo".to_string(), config::LayoutConfig::default());
            let projects_before = state.project_layouts.clone();

            let layout = WindowLayout {
                zen_mode: Some(true),
                ..empty_layout()
            };
            merge_window_layout(layout_to_write(&mut state, None), layout);

            assert!(
                state.layout.zen_mode,
                "the global layout DOES get the write"
            );
            assert_eq!(
                state.project_layouts, projects_before,
                "an ordinary window's write must never reach project_layouts"
            );
        }

        #[test]
        fn an_absent_project_entry_falls_back_to_the_global_layout_on_read() {
            let state = state_with_global(false, 250.0);
            let layout = layout_to_read(&state, Some("/never-opened"));
            assert_eq!(
                layout.bottom_panel_height, 250.0,
                "no entry yet for this project — read the shared layout instead"
            );
            assert!(
                !state.project_layouts.contains_key("/never-opened"),
                "a READ must never create the entry it fell back from"
            );
        }

        #[test]
        fn a_present_project_entry_shadows_the_global_layout_on_read() {
            let mut state = state_with_global(false, 250.0);
            state.project_layouts.insert(
                "/repo".to_string(),
                config::LayoutConfig {
                    bottom_panel_height: 42.0,
                    ..config::LayoutConfig::default()
                },
            );
            let layout = layout_to_read(&state, Some("/repo"));
            assert_eq!(layout.bottom_panel_height, 42.0);
        }

        #[test]
        fn a_non_project_read_always_returns_the_global_layout_regardless_of_project_layouts() {
            let mut state = state_with_global(false, 250.0);
            state
                .project_layouts
                .insert("/repo".to_string(), config::LayoutConfig::default());
            let layout = layout_to_read(&state, None);
            assert_eq!(layout.bottom_panel_height, 250.0);
        }
    }
}
