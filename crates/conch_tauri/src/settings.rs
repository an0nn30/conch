//! Settings dialog Tauri commands.

use conch_core::config::{self, UserConfig};
use serde::Serialize;
use tauri::Emitter;
use ts_rs::TS;

use crate::TauriState;
use crate::theme;

#[derive(Serialize, TS)]
#[ts(export)]
pub(crate) struct SaveSettingsResult {
    restart_required: bool,
}

#[tauri::command]
pub(crate) fn get_all_settings(state: tauri::State<'_, TauriState>) -> serde_json::Value {
    let cfg = state.config.read();
    serde_json::to_value(&*cfg).unwrap_or_default()
}

#[tauri::command]
pub(crate) fn list_themes() -> Vec<String> {
    let mut themes: Vec<String> = conch_core::color_scheme::list_themes()
        .keys()
        .cloned()
        .collect();
    if !themes.iter().any(|t| t == "dracula") {
        themes.push("dracula".into());
    }
    themes.sort();
    themes
}

#[tauri::command]
pub(crate) fn preview_theme_colors(name: String) -> Result<theme::ThemeColors, String> {
    let scheme = conch_core::color_scheme::resolve_theme(&name);
    Ok(theme::resolve_theme_colors_from_scheme(&scheme))
}

#[tauri::command]
pub(crate) fn save_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    settings: serde_json::Value,
) -> Result<SaveSettingsResult, String> {
    let new_config: UserConfig =
        serde_json::from_value(settings).map_err(|e| format!("Invalid settings: {e}"))?;

    let restart_required = {
        let old_config = state.config.read();
        needs_restart(&old_config, &new_config)
    };

    // Update in-memory config before disk write.
    {
        let mut cfg = state.config.write();
        *cfg = new_config.clone();
    }

    config::save_user_config(&new_config).map_err(|e| format!("Failed to save config: {e}"))?;

    let _ = app.emit("config-changed", ());

    // Rebuild menu to pick up keyboard shortcut changes.
    let kb = &new_config.conch.keyboard;
    if let Ok(menu) = crate::menu::build_app_menu(&app, kb) {
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

    // Terminal font
    let old_font = old.resolved_terminal_font();
    let new_font = new.resolved_terminal_font();
    if old_font.normal.family != new_font.normal.family {
        return true;
    }
    if old_font.size != new_font.size {
        return true;
    }
    if old_font.offset.x != new_font.offset.x {
        return true;
    }
    if old_font.offset.y != new_font.offset.y {
        return true;
    }
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
    if old.conch.plugins.enabled != new.conch.plugins.enabled {
        return true;
    }
    if old.conch.plugins.lua != new.conch.plugins.lua {
        return true;
    }
    if old.conch.plugins.java != new.conch.plugins.java {
        return true;
    }
    if old.conch.plugins.search_paths != new.conch.plugins.search_paths {
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
        b.window.decorations = conch_core::config::WindowDecorations::None;
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
    fn changed_terminal_font_needs_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.terminal.font.size = 18.0;
        assert!(needs_restart(&a, &b));
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
        b.conch.keyboard.new_tab = "ctrl+n".into();
        assert!(
            !needs_restart(&a, &b),
            "Keyboard shortcuts are hot-reloadable"
        );
    }

    #[test]
    fn changed_plugin_enabled_needs_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.conch.plugins.enabled = false;
        assert!(needs_restart(&a, &b));
    }

    #[test]
    fn changed_ui_font_no_restart() {
        let a = UserConfig::default();
        let mut b = UserConfig::default();
        b.conch.ui.font.small = 10.0;
        assert!(
            !needs_restart(&a, &b),
            "UI chrome font sizes are hot-reloadable"
        );
    }

    #[test]
    fn preview_theme_colors_returns_dracula_defaults() {
        let tc = crate::theme::resolve_theme_colors_from_scheme(
            &conch_core::color_scheme::resolve_theme("dracula"),
        );
        assert_eq!(tc.background, "#282a36");
        assert_eq!(tc.red, "#ff5555");
    }

    #[test]
    fn preview_theme_colors_unknown_falls_back_to_dracula() {
        let tc = crate::theme::resolve_theme_colors_from_scheme(
            &conch_core::color_scheme::resolve_theme("nonexistent_theme_xyz"),
        );
        // Should fall back to Dracula
        assert_eq!(tc.background, "#282a36");
    }
}
