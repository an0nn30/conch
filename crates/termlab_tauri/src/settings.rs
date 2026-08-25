//! Settings dialog Tauri commands.

use parking_lot::Mutex;
use serde::Serialize;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::Emitter;
use termlab_core::config::{self, UserConfig};
use ts_rs::TS;

use crate::TauriState;
use crate::lsp::commands::LspState;
use crate::lsp::manager::{Enablement, LspManagerHandle, ManagerError};
use crate::plugins::PluginState;

const LIVE_POLICY_APPLY_RETRY: Duration = Duration::from_millis(10);
const LIVE_POLICY_APPLY_DEADLINE: Duration = Duration::from_secs(1);

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

#[derive(Clone, Default)]
pub(crate) struct SettingsTransactionGate {
    busy: Arc<AtomicBool>,
}

struct SettingsTransactionPermit(Arc<AtomicBool>);

impl SettingsTransactionGate {
    fn try_enter(&self) -> Result<SettingsTransactionPermit, String> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| SettingsTransactionPermit(self.busy.clone()))
            .map_err(|_| "A settings save is already in progress".into())
    }
}

impl Drop for SettingsTransactionPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

async fn apply_live_enablement(
    manager: LspManagerHandle,
    enablement: Enablement,
) -> Result<(), ManagerError> {
    let apply_manager = manager.clone();
    converge_live_enablement(
        enablement,
        move |enablement| {
            let manager = apply_manager.clone();
            async move { manager.set_enablement(enablement).await }
        },
        move || async move { manager.shutdown().await },
    )
    .await
}

async fn converge_live_enablement<A, F, Q, QF>(
    enablement: Enablement,
    mut apply: A,
    quarantine: Q,
) -> Result<(), ManagerError>
where
    A: FnMut(Enablement) -> F,
    F: Future<Output = Result<(), ManagerError>>,
    Q: FnOnce() -> QF,
    QF: Future<Output = Result<(), ManagerError>>,
{
    enum Failure {
        Deadline,
        Stopping,
        Unexpected(ManagerError),
    }

    let convergence = tokio::time::timeout(LIVE_POLICY_APPLY_DEADLINE, async {
        loop {
            match apply(enablement.clone()).await {
                Ok(()) => return Ok(()),
                Err(ManagerError::Overloaded) => {
                    tokio::time::sleep(LIVE_POLICY_APPLY_RETRY).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await;
    let failure = match convergence {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(ManagerError::ActorStopped)) => Failure::Stopping,
        Ok(Err(error)) => Failure::Unexpected(error),
        Err(_) => Failure::Deadline,
    };

    // `ActorStopped` can mean shutdown has merely begun. The priority handle
    // joins that generation's terminal fact; overload exhaustion and an
    // unexpected rejection use the same path to quarantine stale authority.
    match (failure, quarantine().await) {
        (Failure::Deadline | Failure::Stopping, Ok(()) | Err(ManagerError::ActorStopped)) => Ok(()),
        (Failure::Unexpected(error), Ok(()) | Err(ManagerError::ActorStopped)) => Err(error),
        (Failure::Deadline, Err(shutdown)) => Err(ManagerError::Infrastructure(format!(
            "live policy deadline elapsed; runtime quarantine failed ({shutdown})"
        ))),
        (Failure::Stopping, Err(shutdown)) => Err(ManagerError::Infrastructure(format!(
            "live runtime was stopping; runtime quarantine failed ({shutdown})"
        ))),
        (Failure::Unexpected(error), Err(shutdown)) => Err(ManagerError::Infrastructure(format!(
            "live policy failed ({error}); runtime quarantine failed ({shutdown})"
        ))),
    }
}

async fn transition_settings_owned<S, A, F, H, HF>(
    gate: &SettingsTransactionGate,
    authority: Arc<parking_lot::RwLock<UserConfig>>,
    new_config: UserConfig,
    save: &S,
    apply_runtime: A,
    apply_hot: H,
) -> Result<(), String>
where
    S: Fn(&UserConfig) -> Result<(), String>,
    A: FnOnce(Enablement) -> F + Send + 'static,
    F: Future<Output = Result<(), ManagerError>> + Send + 'static,
    H: FnOnce(UserConfig) -> HF + Send + 'static,
    HF: Future<Output = Result<(), String>> + Send + 'static,
{
    let permit = gate.try_enter()?;
    save(&new_config).map_err(|error| format!("Failed to save config: {error}"))?;
    let enablement = Enablement::from_config(&new_config.editor.lsp);
    let (reply, result) = tokio::sync::oneshot::channel();

    // Everything after durable persistence is one owned transaction. Invoke
    // cancellation only drops `result`; it cannot skip memory authority,
    // runtime convergence, hot effects, or release the permit early.
    tokio::spawn(async move {
        *authority.write() = new_config.clone();
        let runtime = apply_runtime(enablement).await;
        let hot = apply_hot(new_config).await;
        let outcome = match (runtime, hot) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(runtime), Ok(())) => Err(format!(
                "Settings were saved, but the live LSP policy could not be applied: {runtime}"
            )),
            (Ok(()), Err(hot)) => Err(format!(
                "Settings were saved, but live UI settings could not be applied: {hot}"
            )),
            (Err(runtime), Err(hot)) => Err(format!(
                "Settings were saved, but live LSP policy ({runtime}) and UI settings ({hot}) could not be applied"
            )),
        };
        let _ = reply.send(outcome);
        // `send` is synchronous, so the permit is still held through result
        // delivery and is released before the waiting task can be polled.
        drop(permit);
    });

    result.await.map_err(|_| {
        String::from(
            "Settings were saved, but the owned application transaction stopped unexpectedly",
        )
    })?
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
    transaction_gate: tauri::State<'_, SettingsTransactionGate>,
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
    let authority = state.config.clone();
    let hot_app = app.clone();
    let hot_plugins = Arc::clone(plugin_state.inner());
    transition_settings_owned(
        &transaction_gate,
        authority,
        new_config,
        &|config| config::save_user_config(config).map_err(|error| error.to_string()),
        move |enablement| apply_live_enablement(manager, enablement),
        move |config| async move {
            let mut failures = Vec::new();
            if let Err(error) = hot_app.emit("config-changed", ()) {
                failures.push(format!("config-changed event: {error}"));
            }

            // Rebuild the menu to pick up keyboard shortcuts while preserving
            // dynamically registered plugin menu items. This remains inside
            // the owned transaction so invoke cancellation cannot skip it.
            let plugin_items = hot_plugins.lock().menu_items.read().clone();
            match crate::menu::build_app_menu_with_plugins(
                &hot_app,
                &config.termlab.keyboard,
                &plugin_items,
            ) {
                Ok(menu) => {
                    if let Err(error) = hot_app.set_menu(menu) {
                        failures.push(format!("menu refresh: {error}"));
                    }
                }
                Err(error) => failures.push(format!("menu rebuild: {error}")),
            }

            if failures.is_empty() {
                Ok(())
            } else {
                Err(failures.join("; "))
            }
        },
    )
    .await?;

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
    use crate::lsp::client::ClientEvent;
    use crate::lsp::manager::{
        LspManager, ProjectContextChoice, SessionClient, SessionFactory, SessionStart,
    };
    use crate::lsp::trust::TrustDecision;
    use crate::lsp::types::{DocumentId, ReserveResult};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct ReapDelayedStartingFactory {
        launches: AtomicUsize,
        cleanups_started: AtomicUsize,
        cleanups_completed: AtomicUsize,
        hold_cleanup: AtomicBool,
        cleanup_released: tokio::sync::Notify,
    }

    impl ReapDelayedStartingFactory {
        fn hold_cleanup(&self) {
            self.hold_cleanup.store(true, Ordering::SeqCst);
        }

        async fn wait_for_launch(&self) {
            for _ in 0..100 {
                if self.launches.load(Ordering::SeqCst) > 0 {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("settings fixture never launched its starting generation");
        }

        async fn wait_for_cleanup(&self) {
            for _ in 0..100 {
                if self.cleanups_started.load(Ordering::SeqCst) > 0 {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("settings fixture never entered startup cleanup");
        }

        fn release_cleanup(&self) {
            self.hold_cleanup.store(false, Ordering::SeqCst);
            self.cleanup_released.notify_waiters();
        }
    }

    #[async_trait]
    impl SessionFactory for ReapDelayedStartingFactory {
        async fn start(
            self: Arc<Self>,
            _start: SessionStart,
            _events: tokio::sync::mpsc::Sender<ClientEvent>,
            cancellation: tokio_util::sync::CancellationToken,
        ) -> Result<Arc<dyn SessionClient>, ManagerError> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            cancellation.cancelled().await;
            self.cleanups_started.fetch_add(1, Ordering::SeqCst);
            while self.hold_cleanup.load(Ordering::SeqCst) {
                self.cleanup_released.notified().await;
            }
            self.cleanups_completed.fetch_add(1, Ordering::SeqCst);
            Err(ManagerError::Cancelled)
        }
    }

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
        let state = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let mut new = UserConfig::default();
        new.editor.lsp.enabled = false;
        let applied = Arc::new(Mutex::new(Vec::new()));
        let gate = SettingsTransactionGate::default();
        let applied_for_runtime = applied.clone();
        let authority_for_runtime = state.clone();

        transition_settings_owned(
            &gate,
            state.clone(),
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
                    Ok::<(), ManagerError>(())
                }
            },
            |_| async { Ok(()) },
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
    async fn settings_transaction_preserves_old_authority_only_when_persistence_fails() {
        let state = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let gate = SettingsTransactionGate::default();
        let mut new = UserConfig::default();
        new.editor.lsp.enabled = false;
        let runtime_calls = Arc::new(AtomicUsize::new(0));
        let calls = runtime_calls.clone();
        let error = transition_settings_owned(
            &gate,
            state.clone(),
            new.clone(),
            &|_| Err("disk unavailable".into()),
            move |_| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), ManagerError>(())
                }
            },
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(error.contains("disk unavailable"));
        assert_eq!(runtime_calls.load(Ordering::SeqCst), 0);
        assert!(state.read().editor.lsp.enabled);

        let saves = Arc::new(AtomicUsize::new(0));
        let saves_for_disk = saves.clone();
        transition_settings_owned(
            &gate,
            state.clone(),
            new,
            &move |_| {
                saves_for_disk.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| async { Ok(()) },
            |_| async { Ok(()) },
        )
        .await
        .unwrap();
        assert_eq!(
            saves.load(Ordering::SeqCst),
            1,
            "durable authority is one-way"
        );
        assert!(!state.read().editor.lsp.enabled);
    }

    #[tokio::test]
    async fn runtime_rejection_reports_failure_without_rolling_back_durable_authority() {
        let state = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let gate = SettingsTransactionGate::default();
        let mut new = UserConfig::default();
        new.editor.lsp.enabled = false;
        let saves = AtomicUsize::new(0);
        let error = transition_settings_owned(
            &gate,
            state.clone(),
            new,
            &|_| {
                saves.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| async { Err(ManagerError::Infrastructure("runtime unavailable".into())) },
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(error.contains("Settings were saved"));
        assert!(error.contains("runtime unavailable"));
        assert_eq!(saves.load(Ordering::SeqCst), 1);
        assert!(!state.read().editor.lsp.enabled);
    }

    #[tokio::test]
    async fn concurrent_settings_save_is_explicitly_rejected_without_overtaking() {
        let gate = Arc::new(SettingsTransactionGate::default());
        let state = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let entered_runtime = Arc::new(tokio::sync::Notify::new());
        let release_runtime = Arc::new(tokio::sync::Notify::new());
        let saves = Arc::new(AtomicUsize::new(0));
        let mut disabled = UserConfig::default();
        disabled.editor.lsp.enabled = false;
        let first = tokio::spawn({
            let gate = gate.clone();
            let state = state.clone();
            let entered_runtime = entered_runtime.clone();
            let release_runtime = release_runtime.clone();
            let saves = saves.clone();
            async move {
                transition_settings_owned(
                    &gate,
                    state,
                    disabled,
                    &move |_| {
                        saves.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                    move |_| async move {
                        entered_runtime.notify_one();
                        release_runtime.notified().await;
                        Ok(())
                    },
                    |_| async { Ok(()) },
                )
                .await
            }
        });
        entered_runtime.notified().await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());

        let error = transition_settings_owned(
            &gate,
            state.clone(),
            UserConfig::default(),
            &|_| panic!("busy transaction must not persist"),
            |_| async { Ok(()) },
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(error.contains("already in progress"));
        assert_eq!(saves.load(Ordering::SeqCst), 1);
        assert!(!state.read().editor.lsp.enabled);

        release_runtime.notify_one();
        while gate.busy.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        transition_settings_owned(
            &gate,
            state.clone(),
            UserConfig::default(),
            &|_| Ok(()),
            |_| async { Ok(()) },
            |_| async { Ok(()) },
        )
        .await
        .unwrap();
        assert!(state.read().editor.lsp.enabled);
    }

    #[tokio::test(start_paused = true)]
    async fn sustained_manager_overload_hits_a_deadline_and_quarantines_once() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let quarantines = Arc::new(AtomicUsize::new(0));
        let apply = tokio::spawn({
            let attempts = attempts.clone();
            let quarantines = quarantines.clone();
            async move {
                converge_live_enablement(
                    Enablement::none(),
                    move |_| {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        async { Err(ManagerError::Overloaded) }
                    },
                    move || {
                        quarantines.fetch_add(1, Ordering::SeqCst);
                        async { Ok(()) }
                    },
                )
                .await
            }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(super::LIVE_POLICY_APPLY_DEADLINE).await;
        tokio::task::yield_now().await;

        apply.await.unwrap().unwrap();
        assert!(attempts.load(Ordering::SeqCst) > 0);
        assert!(attempts.load(Ordering::SeqCst) <= 101);
        assert_eq!(quarantines.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn actor_stopped_waits_for_priority_shutdown_before_runtime_is_absent() {
        let entered_shutdown = Arc::new(tokio::sync::Notify::new());
        let release_shutdown = Arc::new(tokio::sync::Notify::new());
        let converging = tokio::spawn({
            let entered_shutdown = entered_shutdown.clone();
            let release_shutdown = release_shutdown.clone();
            async move {
                converge_live_enablement(
                    Enablement::none(),
                    |_| async { Err(ManagerError::ActorStopped) },
                    move || async move {
                        entered_shutdown.notify_one();
                        release_shutdown.notified().await;
                        Ok(())
                    },
                )
                .await
            }
        });

        entered_shutdown.notified().await;
        assert!(
            !converging.is_finished(),
            "ActorStopped did not wait for the live generation's priority shutdown"
        );
        release_shutdown.notify_one();
        converging.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancellation_after_persistence_still_applies_memory_runtime_and_all_hot_effects() {
        let gate = Arc::new(SettingsTransactionGate::default());
        let state = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let persisted = Arc::new(AtomicUsize::new(0));
        let runtime_entered = Arc::new(tokio::sync::Notify::new());
        let runtime_release = Arc::new(tokio::sync::Notify::new());
        let runtime_applied = Arc::new(AtomicUsize::new(0));
        let config_events = Arc::new(AtomicUsize::new(0));
        let menu_refreshes = Arc::new(AtomicUsize::new(0));
        let mut disabled = UserConfig::default();
        disabled.editor.lsp.enabled = false;
        let invoke = tokio::spawn({
            let gate = gate.clone();
            let state = state.clone();
            let persisted = persisted.clone();
            let runtime_entered = runtime_entered.clone();
            let runtime_release = runtime_release.clone();
            let runtime_applied = runtime_applied.clone();
            let config_events = config_events.clone();
            let menu_refreshes = menu_refreshes.clone();
            async move {
                transition_settings_owned(
                    &gate,
                    state,
                    disabled,
                    &move |_| {
                        persisted.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                    move |_| async move {
                        runtime_entered.notify_one();
                        runtime_release.notified().await;
                        runtime_applied.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                    move |_| async move {
                        config_events.fetch_add(1, Ordering::SeqCst);
                        menu_refreshes.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
            }
        });

        runtime_entered.notified().await;
        invoke.abort();
        assert!(invoke.await.unwrap_err().is_cancelled());
        assert_eq!(persisted.load(Ordering::SeqCst), 1);
        assert!(!state.read().editor.lsp.enabled);
        assert!(gate.busy.load(Ordering::Acquire));
        runtime_release.notify_one();
        for _ in 0..100 {
            if !gate.busy.load(Ordering::Acquire) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(runtime_applied.load(Ordering::SeqCst), 1);
        assert_eq!(config_events.load(Ordering::SeqCst), 1);
        assert_eq!(menu_refreshes.load(Ordering::SeqCst), 1);
        assert!(!gate.busy.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn cancelled_settings_invoke_holds_gate_and_hot_effects_until_startup_reap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&cache_root).unwrap();
        let path = root.join("settings-live-start.ts");
        std::fs::write(&path, "let value = 1;").unwrap();
        let factory = Arc::new(ReapDelayedStartingFactory::default());
        factory.hold_cleanup();
        let (manager, actor, _events) =
            LspManager::new(factory.clone(), config_root, cache_root, Enablement::all());
        tokio::spawn(actor.run());

        let reservation = manager
            .reserve_document(path.clone(), "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = reservation else {
            panic!("fresh settings fixture path must reserve")
        };
        let opened = manager
            .open_document(
                reservation_id,
                "pane".into(),
                "let value = 1;".into(),
                "typescript".into(),
            )
            .await
            .unwrap();
        let document: DocumentId =
            serde_json::from_value(serde_json::Value::String(opened.document_id)).unwrap();
        manager
            .set_project_context(document, ProjectContextChoice::root(root.clone()))
            .await
            .unwrap();
        manager
            .set_project_trust(root, Some("typescript".into()), TrustDecision::Trusted)
            .await
            .unwrap();
        factory.wait_for_launch().await;

        let primary_shutdown = tokio::spawn({
            let manager = manager.clone();
            async move { manager.shutdown().await }
        });
        factory.wait_for_cleanup().await;

        let gate = Arc::new(SettingsTransactionGate::default());
        let authority = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let hot_effects = Arc::new(AtomicUsize::new(0));
        let mut disabled = UserConfig::default();
        disabled.editor.lsp.enabled = false;
        let invoke = tokio::spawn({
            let gate = gate.clone();
            let authority = authority.clone();
            let manager = manager.clone();
            let hot_effects = hot_effects.clone();
            async move {
                transition_settings_owned(
                    &gate,
                    authority,
                    disabled,
                    &|_| Ok(()),
                    move |enablement| apply_live_enablement(manager, enablement),
                    move |_| async move {
                        hot_effects.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
            }
        });
        for _ in 0..100 {
            if !authority.read().editor.lsp.enabled {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!authority.read().editor.lsp.enabled);
        invoke.abort();
        assert!(invoke.await.unwrap_err().is_cancelled());
        assert!(gate.busy.load(Ordering::Acquire));
        assert_eq!(hot_effects.load(Ordering::SeqCst), 0);
        assert!(
            !primary_shutdown.is_finished(),
            "priority shutdown reported convergence before the starting generation reaped"
        );

        factory.release_cleanup();
        primary_shutdown.await.unwrap().unwrap();
        for _ in 0..100 {
            if !gate.busy.load(Ordering::Acquire) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(factory.cleanups_completed.load(Ordering::SeqCst), 1);
        assert_eq!(hot_effects.load(Ordering::SeqCst), 1);
        assert!(!gate.busy.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn transaction_permit_is_held_until_hot_application_finishes() {
        let gate = Arc::new(SettingsTransactionGate::default());
        let state = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let hot_entered = Arc::new(tokio::sync::Notify::new());
        let hot_release = Arc::new(tokio::sync::Notify::new());
        let first = tokio::spawn({
            let gate = gate.clone();
            let state = state.clone();
            let hot_entered = hot_entered.clone();
            let hot_release = hot_release.clone();
            async move {
                transition_settings_owned(
                    &gate,
                    state,
                    UserConfig::default(),
                    &|_| Ok(()),
                    |_| async { Ok(()) },
                    move |_| async move {
                        hot_entered.notify_one();
                        hot_release.notified().await;
                        Ok(())
                    },
                )
                .await
            }
        });
        hot_entered.notified().await;

        let busy = transition_settings_owned(
            &gate,
            state.clone(),
            UserConfig::default(),
            &|_| panic!("busy transaction must not persist"),
            |_| async { Ok(()) },
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(busy.contains("already in progress"));
        hot_release.notify_one();
        first.await.unwrap().unwrap();
        transition_settings_owned(
            &gate,
            state,
            UserConfig::default(),
            &|_| Ok(()),
            |_| async { Ok(()) },
            |_| async { Ok(()) },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn persistence_failure_leaves_old_memory_and_application_effects_untouched() {
        let gate = SettingsTransactionGate::default();
        let state = Arc::new(parking_lot::RwLock::new(UserConfig::default()));
        let runtime = Arc::new(AtomicUsize::new(0));
        let hot = Arc::new(AtomicUsize::new(0));
        let mut disabled = UserConfig::default();
        disabled.editor.lsp.enabled = false;
        let result = transition_settings_owned(
            &gate,
            state.clone(),
            disabled,
            &|_| Err("disk unavailable".into()),
            {
                let runtime = runtime.clone();
                move |_| async move {
                    runtime.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            {
                let hot = hot.clone();
                move |_| async move {
                    hot.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await;

        assert!(result.unwrap_err().contains("disk unavailable"));
        assert!(state.read().editor.lsp.enabled);
        assert_eq!(runtime.load(Ordering::SeqCst), 0);
        assert_eq!(hot.load(Ordering::SeqCst), 0);
        assert!(!gate.busy.load(Ordering::Acquire));
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
