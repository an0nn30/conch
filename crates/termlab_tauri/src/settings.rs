//! Settings dialog Tauri commands.

use parking_lot::Mutex;
use serde::Serialize;
use std::future::Future;
use std::sync::Arc;
use tauri::Emitter;
use termlab_core::config::{self, UserConfig};
use ts_rs::TS;

use crate::TauriState;
use crate::lsp::commands::LspState;
use crate::lsp::manager::Enablement;
use crate::plugins::PluginState;

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

async fn transition_settings<S, A, F>(
    authority: &parking_lot::RwLock<UserConfig>,
    new_config: UserConfig,
    save: &S,
    apply_runtime: A,
) -> Result<(), String>
where
    S: Fn(&UserConfig) -> Result<(), String>,
    A: FnOnce(Enablement) -> F,
    F: Future<Output = Result<(), String>>,
{
    let old_config = authority.read().clone();
    save(&new_config).map_err(|error| format!("Failed to save config: {error}"))?;
    if let Err(manager_error) = apply_runtime(Enablement::from_config(&new_config.editor.lsp)).await
    {
        return match save(&old_config) {
            Ok(()) => Err(format!("Failed to apply LSP settings: {manager_error}")),
            Err(rollback_error) => {
                // The new file is the only durable authority left. The actor
                // rejected the transition because it is unavailable, so keep
                // in-memory settings aligned with disk and report both facts.
                *authority.write() = new_config;
                Err(format!(
                    "Failed to apply LSP settings: {manager_error}; rollback failed: {rollback_error}; persisted settings retained and LSP runtime unavailable"
                ))
            }
        };
    }
    *authority.write() = new_config;
    Ok(())
}

#[tauri::command]
pub(crate) fn get_all_settings(state: tauri::State<'_, TauriState>) -> serde_json::Value {
    let mut cfg = state.config.read().clone();
    normalize_plugin_search_paths(&mut cfg);
    serde_json::to_value(cfg).unwrap_or_default()
}

#[tauri::command]
pub(crate) async fn save_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, TauriState>,
    lsp_state: tauri::State<'_, LspState>,
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

    let manager = lsp_state.manager().clone();
    transition_settings(
        &state.config,
        new_config.clone(),
        &|config| config::save_user_config(config).map_err(|error| error.to_string()),
        move |enablement| async move {
            manager
                .set_enablement(enablement)
                .await
                .map_err(|error| error.to_string())
        },
    )
    .await?;

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
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn changed_lsp_enablement_is_a_live_runtime_policy() {
        let old = UserConfig::default();
        let mut new = old.clone();
        new.editor.lsp.enabled = !old.editor.lsp.enabled;
        assert!(!needs_restart(&old, &new));
        let policy = Enablement::from_config(&new.editor.lsp);
        assert_eq!(
            policy.enables("typescript"),
            new.editor.lsp.enabled && new.editor.lsp.languages.typescript
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

    #[tokio::test]
    async fn settings_transaction_persists_and_applies_live_policy_without_holding_config_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.toml");
        let state = parking_lot::RwLock::new(UserConfig::default());
        let mut new = UserConfig::default();
        new.editor.lsp.enabled = false;
        let applied = Arc::new(Mutex::new(Vec::new()));
        let applied_for_runtime = applied.clone();
        let authority_for_runtime = &state;

        transition_settings(
            &state,
            new,
            &|config| {
                std::fs::write(&path, toml::to_string(config).unwrap())
                    .map_err(|error| error.to_string())
            },
            move |policy| {
                assert!(
                    authority_for_runtime.try_write().is_some(),
                    "config guard must be released before awaiting the manager"
                );
                let applied = applied_for_runtime.clone();
                async move {
                    applied.lock().push(policy);
                    Ok(())
                }
            },
        )
        .await
        .unwrap();

        let persisted: UserConfig =
            toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(!persisted.editor.lsp.enabled);
        assert!(!state.read().editor.lsp.enabled);
        assert!(!applied.lock()[0].enables("typescript"));
    }

    #[tokio::test]
    async fn settings_transaction_preserves_old_authority_on_save_or_manager_failure() {
        let state = parking_lot::RwLock::new(UserConfig::default());
        let mut new = UserConfig::default();
        new.editor.lsp.enabled = false;
        let runtime_calls = Arc::new(AtomicUsize::new(0));
        let calls = runtime_calls.clone();
        let error = transition_settings(
            &state,
            new.clone(),
            &|_| Err("disk unavailable".into()),
            move |_| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await
        .unwrap_err();
        assert!(error.contains("disk unavailable"));
        assert_eq!(runtime_calls.load(Ordering::SeqCst), 0);
        assert!(state.read().editor.lsp.enabled);

        let saves = Arc::new(AtomicUsize::new(0));
        let saves_for_disk = saves.clone();
        let error = transition_settings(
            &state,
            new,
            &move |_| {
                saves_for_disk.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| async { Err("manager stopped".into()) },
        )
        .await
        .unwrap_err();
        assert!(error.contains("manager stopped"));
        assert_eq!(saves.load(Ordering::SeqCst), 2, "new save plus rollback");
        assert!(state.read().editor.lsp.enabled);
    }

    #[tokio::test]
    async fn settings_transaction_surfaces_rollback_failure_and_keeps_memory_with_durable_disk() {
        let state = parking_lot::RwLock::new(UserConfig::default());
        let mut new = UserConfig::default();
        new.editor.lsp.enabled = false;
        let saves = AtomicUsize::new(0);
        let error = transition_settings(
            &state,
            new,
            &|_| {
                if saves.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err("rollback disk full".into())
                }
            },
            |_| async { Err("manager stopped".into()) },
        )
        .await
        .unwrap_err();
        assert!(error.contains("manager stopped"));
        assert!(error.contains("rollback disk full"));
        assert!(error.contains("persisted settings retained"));
        assert!(!state.read().editor.lsp.enabled);
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
