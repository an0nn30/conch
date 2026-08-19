//! Settings dialog Tauri commands.

use termlab_core::config::{self, UserConfig};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use tauri::Emitter;
use ts_rs::TS;

use crate::TauriState;
use crate::plugins::PluginState;
use crate::theme;

fn normalize_plugin_search_paths(cfg: &mut UserConfig) {
    let config_dir = termlab_core::config::config_dir();
    let legacy_abs = config_dir.join("plugins_v2");
    let current_abs = config_dir.join("plugins");

    for path in &mut cfg.termlab.plugins.search_paths {
        let trimmed = path.trim();
        if trimmed == "~/.config/termlab/plugins_v2" {
            *path = "~/.config/termlab/plugins".to_string();
            continue;
        }

        let as_path = std::path::Path::new(trimmed);
        if as_path == legacy_abs {
            *path = current_abs.to_string_lossy().to_string();
        }
    }
}

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct SaveSettingsResult {
    restart_required: bool,
}

#[tauri::command]
pub(crate) fn get_all_settings(state: tauri::State<'_, TauriState>) -> serde_json::Value {
    let mut cfg = state.config.read().clone();
    normalize_plugin_search_paths(&mut cfg);
    serde_json::to_value(cfg).unwrap_or_default()
}

/// Theme names for the settings picker.
///
/// Does NOT read `colors.theme` — it enumerates the theme directories — so it
/// needs no appearance resolution. It does have to OFFER the reserved `auto`
/// name, though, now that `auto` is the default `colors.theme`: without it the
/// picker could not display (or re-select) the value a fresh config carries.
/// Same shape as the `dracula` guarantee below, which exists for the same
/// reason (the built-in fallback is always selectable even with no file on
/// disk). `auto` is prepended rather than sorted in, so it reads as the
/// distinct reserved entry it is.
#[tauri::command]
pub(crate) fn list_themes() -> Vec<String> {
    let mut themes: Vec<String> = termlab_core::color_scheme::list_themes()
        .keys()
        .cloned()
        .collect();
    if !themes.iter().any(|t| t == "dracula") {
        themes.push("dracula".into());
    }
    themes.sort();
    themes.insert(0, termlab_core::effective_theme::AUTO_THEME_NAME.into());
    themes
}

/// Preview a theme BY NAME (the picker's current selection, not the saved
/// config), so it does not read `colors.theme` either. It must still honor
/// `auto`, because `list_themes` now offers it: previewing `auto` without
/// resolution would fall through the name lookup and silently show Dracula.
///
/// `resolved_appearance` is optional and defaults to dark, exactly as in
/// `commands::get_theme_colors`.
#[tauri::command]
pub(crate) fn preview_theme_colors(
    name: String,
    resolved_appearance: Option<String>,
) -> Result<theme::ThemeColors, String> {
    let effective = termlab_core::effective_theme::effective_theme_name(
        &name,
        resolved_appearance
            .as_deref()
            .unwrap_or(termlab_core::effective_theme::DEFAULT_RESOLVED_APPEARANCE),
    );
    let scheme = termlab_core::color_scheme::resolve_theme(effective);
    Ok(theme::resolve_theme_colors_from_scheme(&scheme))
}

#[tauri::command]
pub(crate) fn save_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    plugin_state: tauri::State<'_, Arc<Mutex<PluginState>>>,
    settings: serde_json::Value,
) -> Result<SaveSettingsResult, String> {
    let mut new_config: UserConfig =
        serde_json::from_value(settings).map_err(|e| format!("Invalid settings: {e}"))?;
    normalize_plugin_search_paths(&mut new_config);

    let restart_required = {
        let mut old_config = state.config.read().clone();
        normalize_plugin_search_paths(&mut old_config);
        needs_restart(&old_config, &new_config)
    };

    // Update in-memory config before disk write.
    {
        let mut cfg = state.config.write();
        *cfg = new_config.clone();
    }

    config::save_user_config(&new_config).map_err(|e| format!("Failed to save config: {e}"))?;

    let _ = app.emit("config-changed", ());

    // Rebuild menu to pick up keyboard shortcut changes while preserving
    // dynamically registered plugin menu items.
    let kb = &new_config.termlab.keyboard;
    let plugin_items = plugin_state.lock().menu_items.read().clone();
    if let Ok(menu) = crate::menu::build_app_menu_with_plugins(&app, kb, &plugin_items) {
        let _ = app.set_menu(menu);
    }

    Ok(SaveSettingsResult { restart_required })
}

/// Compare two configs and return true if any restart-required field differs.
pub(crate) fn needs_restart(old: &UserConfig, new: &UserConfig) -> bool {
    // Window
    if old.window.decorations != new.window.decorations {
        return true;
    }
    if old.window.dimensions.columns != new.window.dimensions.columns {
        return true;
    }
    if old.window.dimensions.lines != new.window.dimensions.lines {
        return true;
    }

    // Terminal font — hot-reloaded via config-changed event, no restart needed.

    // Scroll sensitivity
    if old.terminal.scroll_sensitivity != new.terminal.scroll_sensitivity {
        return true;
    }

    // Shell
    if old.terminal.shell.program != new.terminal.shell.program {
        return true;
    }
    if old.terminal.shell.args != new.terminal.shell.args {
        return true;
    }
    if old.terminal.env != new.terminal.env {
        return true;
    }

    // Cursor
    if old.terminal.cursor != new.terminal.cursor {
        return true;
    }

    // UI chrome fonts — hot-reloaded via config-changed event, no restart needed.

    // Plugins
    if old.termlab.plugins.enabled != new.termlab.plugins.enabled {
        return true;
    }
    if old.termlab.plugins.lua != new.termlab.plugins.lua {
        return true;
    }
    if old.termlab.plugins.java != new.termlab.plugins.java {
        return true;
    }
    if old.termlab.plugins.search_paths != new.termlab.plugins.search_paths {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_configs_no_restart() {
        let a = UserConfig::default();
        let b = UserConfig::default();
        assert!(!needs_restart(&a, &b));
    }

    #[test]
    fn changed_decorations_needs_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.window.decorations = termlab_core::config::WindowDecorations::None;
        assert!(needs_restart(&a, &b));
    }

    #[test]
    fn changed_theme_no_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.colors.theme = "monokai".into();
        assert!(
            !needs_restart(&a, &b),
            "Theme is hot-reloadable, should not require restart"
        );
    }

    #[test]
    fn changed_terminal_font_no_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.terminal.font.size = 18.0;
        assert!(
            !needs_restart(&a, &b),
            "Terminal font is hot-reloadable, should not require restart"
        );
    }

    #[test]
    fn changed_shell_program_needs_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.terminal.shell.program = "/bin/bash".into();
        assert!(needs_restart(&a, &b));
    }

    #[test]
    fn changed_keyboard_shortcut_no_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.termlab.keyboard.new_tab = "ctrl+n".into();
        assert!(
            !needs_restart(&a, &b),
            "Keyboard shortcuts are hot-reloadable"
        );
    }

    #[test]
    fn changed_vim_mode_no_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.editor.vim_mode = true;
        assert!(
            !needs_restart(&a, &b),
            "vim mode is applied live via config-changed; asking for a restart would be a lie"
        );
    }

    #[test]
    fn changed_plugin_enabled_needs_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.termlab.plugins.enabled = false;
        assert!(needs_restart(&a, &b));
    }

    #[test]
    fn changed_ui_font_no_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.termlab.ui.font.small = 10.0;
        assert!(
            !needs_restart(&a, &b),
            "UI chrome font sizes are hot-reloadable"
        );
    }

    // -----------------------------------------------------------------
    // list_themes / preview_theme_colors
    //
    // These call the command functions themselves. An earlier version of
    // these tests re-implemented the command bodies (`resolve_theme(name)`
    // inline), which meant they kept passing no matter what the commands
    // actually did — they were decoys. Everything below goes through
    // `super::list_themes()` / `super::preview_theme_colors(..)` so that
    // deleting either the `auto` prepend or the effective-resolution routing
    // reds a named test.
    // -----------------------------------------------------------------

    /// Background of a theme resolved the way the terminal itself resolves
    /// it — the reference the previews are checked against, so a preview
    /// cannot drift from what the pane will show.
    fn terminal_background(name: &str) -> String {
        crate::theme::resolve_theme_colors_from_scheme(&termlab_core::color_scheme::resolve_theme(
            name,
        ))
        .background
    }

    /// `auto` is a reserved name with no file behind it, so the picker can
    /// only offer it if `list_themes` puts it there. Without this the
    /// settings picker cannot display (or re-select) the value a fresh
    /// config carries, since `auto` is now the default `colors.theme`.
    #[test]
    fn list_themes_offers_the_reserved_auto_name_first() {
        let themes = list_themes();
        assert_eq!(
            themes.first().map(String::as_str),
            Some(termlab_core::effective_theme::AUTO_THEME_NAME),
            "`auto` must be offered, and offered first"
        );
    }

    /// The rest of the list keeps the pre-existing sorted-with-dracula
    /// shape: `auto` is *prepended*, not sorted in, so it reads as the
    /// distinct reserved entry it is.
    #[test]
    fn list_themes_keeps_the_remaining_entries_sorted_and_includes_dracula() {
        let themes = list_themes();
        let rest = &themes[1..];
        let mut sorted = rest.to_vec();
        sorted.sort();
        assert_eq!(rest, sorted.as_slice(), "the tail is still sorted");
        assert!(
            themes.iter().any(|t| t == "dracula"),
            "the built-in fallback is always selectable"
        );
    }

    /// The routing under test: previewing `auto` must resolve it against the
    /// appearance. Without the routing, `resolve_theme("auto")` finds no such
    /// file and silently returns Dracula — so the picker would show Dracula
    /// while the terminal shows a TermLab built-in.
    #[test]
    fn preview_theme_colors_resolves_auto_against_the_appearance() {
        let dark = preview_theme_colors("auto".into(), Some("dark".into())).unwrap();
        let light = preview_theme_colors("auto".into(), Some("light".into())).unwrap();

        assert_eq!(
            dark.background,
            terminal_background(termlab_core::effective_theme::TERMLAB_DARK_THEME),
            "auto/dark previews exactly what the terminal will show"
        );
        assert_eq!(
            light.background,
            terminal_background(termlab_core::effective_theme::TERMLAB_LIGHT_THEME),
            "auto/light previews exactly what the terminal will show"
        );
        assert_ne!(
            dark.background, light.background,
            "auto must track the appearance"
        );
        // Stated negatively too, because Dracula is precisely what an
        // unrouted `auto` would silently produce.
        assert_ne!(
            dark.background, "#282a36",
            "not the unresolved-name fallback"
        );
        assert_ne!(
            light.background, "#282a36",
            "not the unresolved-name fallback"
        );
    }

    /// Back-compat, matching `commands::get_theme_colors`: an omitted
    /// appearance argument means dark.
    #[test]
    fn preview_theme_colors_defaults_to_dark_without_an_appearance() {
        let absent = preview_theme_colors("auto".into(), None).unwrap();
        assert_eq!(
            absent.background,
            terminal_background(termlab_core::effective_theme::TERMLAB_DARK_THEME)
        );
    }

    /// Decoupling: a concrete name previews identically under both
    /// appearances, so a Gruvbox user's preview does not shift on a flip.
    #[test]
    fn preview_theme_colors_is_appearance_independent_for_a_concrete_name() {
        let dark = preview_theme_colors("dracula".into(), Some("dark".into())).unwrap();
        let light = preview_theme_colors("dracula".into(), Some("light".into())).unwrap();
        assert_eq!(dark.background, light.background);
        assert_eq!(dark.background, "#282a36");
    }

    #[test]
    fn preview_theme_colors_returns_dracula_defaults() {
        let tc = preview_theme_colors("dracula".into(), None).unwrap();
        assert_eq!(tc.background, "#282a36");
        assert_eq!(tc.red, "#ff5555");
    }

    #[test]
    fn preview_theme_colors_unknown_falls_back_to_dracula() {
        let tc = preview_theme_colors("nonexistent_theme_xyz".into(), None).unwrap();
        // Should fall back to Dracula
        assert_eq!(tc.background, "#282a36");
    }

    #[test]
    fn normalize_legacy_plugin_path_tilde_form() {
        let mut cfg = UserConfig::default();
        cfg.termlab.plugins.search_paths = vec!["~/.config/termlab/plugins_v2".into()];
        normalize_plugin_search_paths(&mut cfg);
        assert_eq!(
            cfg.termlab.plugins.search_paths,
            vec!["~/.config/termlab/plugins".to_string()]
        );
    }

    #[test]
    fn normalize_legacy_plugin_path_absolute_form() {
        let mut cfg = UserConfig::default();
        let legacy = termlab_core::config::config_dir().join("plugins_v2");
        let current = termlab_core::config::config_dir().join("plugins");
        cfg.termlab.plugins.search_paths = vec![legacy.to_string_lossy().to_string()];
        normalize_plugin_search_paths(&mut cfg);
        assert_eq!(
            cfg.termlab.plugins.search_paths,
            vec![current.to_string_lossy().to_string()]
        );
    }
}
