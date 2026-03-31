//! TauriHostApi — implements the HostApi trait for the Tauri webview UI.
//!
//! Each plugin gets its own `TauriHostApi` instance. Widget updates, events,
//! notifications, and dialogs are forwarded to the Tauri frontend via events.

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use conch_plugin::HostApi;
use conch_plugin::bus::PluginBus;
use conch_plugin_sdk::PanelLocation;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use super::{PanelInfo, PendingDialogs, PluginMenuItem, PluginViewInfo};

static NEXT_PANEL_HANDLE: AtomicU64 = AtomicU64::new(1);
static NEXT_DOCKED_VIEW_ID: AtomicU64 = AtomicU64::new(1);

/// Per-plugin HostApi implementation for the Tauri UI.
pub(crate) struct TauriHostApi {
    pub name: String,
    pub app_handle: tauri::AppHandle,
    pub bus: std::sync::Arc<PluginBus>,
    pub panels: std::sync::Arc<RwLock<HashMap<u64, PanelInfo>>>,
    pub views_by_id: std::sync::Arc<RwLock<HashMap<String, PluginViewInfo>>>,
    pub pane_to_view: std::sync::Arc<RwLock<HashMap<u32, String>>>,
    pub menu_items: std::sync::Arc<RwLock<Vec<PluginMenuItem>>>,
    pub pending_dialogs: std::sync::Arc<Mutex<PendingDialogs>>,
}

impl TauriHostApi {
    fn active_window_and_pane(&self) -> Option<(String, u32)> {
        let tauri_state = self.app_handle.state::<crate::TauriState>();
        let active = tauri_state.active_panes.lock();
        if active.is_empty() {
            return None;
        }
        let label = if active.contains_key("main") {
            "main".to_string()
        } else {
            active.keys().next()?.clone()
        };
        let pane_id = *active.get(&label)?;
        Some((label, pane_id))
    }
}

// -- Tauri events emitted by TauriHostApi --

#[derive(Clone, Serialize)]
struct PluginPanelRegistered {
    handle: u64,
    plugin: String,
    name: String,
    location: String,
    icon: Option<String>,
}

#[derive(Clone, Serialize)]
struct PluginWidgetsUpdated {
    handle: u64,
    plugin: String,
    widgets_json: String,
}

#[derive(Clone, Serialize)]
struct PluginViewOpenRequested {
    plugin: String,
    view_id: String,
    request_json: String,
}

#[derive(Clone, Serialize)]
struct PluginViewFocusRequested {
    plugin: String,
    view_id: String,
}

#[derive(Clone, Serialize)]
struct PluginViewCloseRequested {
    plugin: String,
    view_id: String,
}

#[derive(Debug, Deserialize)]
struct OpenDockedViewRequest {
    id: Option<String>,
    title: Option<String>,
    icon: Option<String>,
}

#[derive(Clone, Serialize)]
struct PluginNotification {
    plugin: String,
    json: String,
}

#[derive(Clone, Serialize)]
struct PluginLogMessage {
    plugin: String,
    level: u8,
    msg: String,
}

#[derive(Clone, Serialize)]
struct PluginStatusUpdate {
    plugin: String,
    text: Option<String>,
    level: u8,
    progress: f32,
}

impl HostApi for TauriHostApi {
    fn plugin_name(&self) -> &str {
        &self.name
    }

    fn register_panel(&self, location: PanelLocation, name: &str, icon: Option<&str>) -> u64 {
        let handle = NEXT_PANEL_HANDLE.fetch_add(1, Ordering::Relaxed);
        let loc_str = match location {
            PanelLocation::Left => "left",
            PanelLocation::Right => "right",
            PanelLocation::Bottom => "bottom",
            _ => "right",
        };

        self.panels.write().insert(
            handle,
            PanelInfo {
                plugin_name: self.name.clone(),
                panel_name: name.to_string(),
                location: loc_str.to_string(),
                icon: icon.map(String::from),
                widgets_json: "[]".to_string(),
            },
        );

        let _ = self.app_handle.emit(
            "plugin-panel-registered",
            PluginPanelRegistered {
                handle,
                plugin: self.name.clone(),
                name: name.to_string(),
                location: loc_str.to_string(),
                icon: icon.map(String::from),
            },
        );

        handle
    }

    fn set_widgets(&self, handle: u64, widgets_json: &str) {
        if let Some(panel) = self.panels.write().get_mut(&handle) {
            panel.widgets_json = widgets_json.to_string();
        }

        let _ = self.app_handle.emit(
            "plugin-widgets-updated",
            PluginWidgetsUpdated {
                handle,
                plugin: self.name.clone(),
                widgets_json: widgets_json.to_string(),
            },
        );
    }

    fn open_docked_view(&self, req_json: &str) -> Option<String> {
        let req: OpenDockedViewRequest =
            serde_json::from_str(req_json).unwrap_or(OpenDockedViewRequest {
                id: None,
                title: None,
                icon: None,
            });
        let stable = req.id.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let view_id = if let Some(stable_id) = stable {
            format!("plugin:{}:view:{}", self.name, stable_id)
        } else {
            let n = NEXT_DOCKED_VIEW_ID.fetch_add(1, Ordering::Relaxed);
            format!("plugin:{}:view:{n}", self.name)
        };

        // v1 scaffolding: tab/pane binding is completed by the frontend in Phase D.
        if let Some(existing) = self.views_by_id.read().get(&view_id) {
            let _ = self.app_handle.emit(
                "plugin-view-focus-requested",
                PluginViewFocusRequested {
                    plugin: self.name.clone(),
                    view_id: existing.view_id.clone(),
                },
            );
            return Some(
                serde_json::json!({
                    "view_id": existing.view_id,
                    "pane_id": existing.pane_id,
                    "tab_id": existing.tab_id,
                })
                .to_string(),
            );
        }

        let info = PluginViewInfo {
            view_id: view_id.clone(),
            plugin_name: self.name.clone(),
            pane_id: 0,
            tab_id: 0,
            title: req.title.unwrap_or_else(|| self.name.clone()),
            icon: req.icon,
            last_widgets_json: "[]".to_string(),
        };
        self.views_by_id.write().insert(view_id.clone(), info);

        let _ = self.app_handle.emit(
            "plugin-view-open-requested",
            PluginViewOpenRequested {
                plugin: self.name.clone(),
                view_id: view_id.clone(),
                request_json: req_json.to_string(),
            },
        );

        Some(
            serde_json::json!({
                "view_id": view_id,
                "pane_id": 0,
                "tab_id": 0,
            })
            .to_string(),
        )
    }

    fn close_docked_view(&self, view_id: &str) -> bool {
        let mut removed = false;
        if let Some(info) = self.views_by_id.write().remove(view_id) {
            if info.pane_id != 0 {
                self.pane_to_view.write().remove(&info.pane_id);
            }
            removed = true;
        }
        let _ = self.app_handle.emit(
            "plugin-view-close-requested",
            PluginViewCloseRequested {
                plugin: self.name.clone(),
                view_id: view_id.to_string(),
            },
        );
        removed
    }

    fn focus_docked_view(&self, view_id: &str) -> bool {
        if !self.views_by_id.read().contains_key(view_id) {
            return false;
        }
        let _ = self.app_handle.emit(
            "plugin-view-focus-requested",
            PluginViewFocusRequested {
                plugin: self.name.clone(),
                view_id: view_id.to_string(),
            },
        );
        true
    }

    fn log(&self, level: u8, msg: &str) {
        let level_str = match level {
            0 => "TRACE",
            1 => "DEBUG",
            2 => "INFO",
            3 => "WARN",
            4 => "ERROR",
            _ => "INFO",
        };
        log::log!(
            match level {
                0 => log::Level::Trace,
                1 => log::Level::Debug,
                2 => log::Level::Info,
                3 => log::Level::Warn,
                4 | _ => log::Level::Error,
            },
            "[plugin:{}] {msg}",
            self.name
        );
    }

    fn notify(&self, json: &str) {
        let _ = self.app_handle.emit(
            "plugin-notification",
            PluginNotification {
                plugin: self.name.clone(),
                json: json.to_string(),
            },
        );
    }

    fn set_status(&self, text: Option<&str>, level: u8, progress: f32) {
        let _ = self.app_handle.emit(
            "plugin-status",
            PluginStatusUpdate {
                plugin: self.name.clone(),
                text: text.map(String::from),
                level,
                progress,
            },
        );
    }

    fn publish_event(&self, event_type: &str, data_json: &str) {
        let data: serde_json::Value =
            serde_json::from_str(data_json).unwrap_or(serde_json::Value::Null);
        self.bus.publish(&self.name, event_type, data);
    }

    fn subscribe(&self, event_type: &str) {
        self.bus.subscribe(&self.name, event_type);
    }

    fn query_plugin(&self, target: &str, method: &str, args_json: &str) -> Option<String> {
        let args: serde_json::Value =
            serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
        match self.bus.query_blocking(target, method, args, &self.name) {
            Ok(resp) => match resp.result {
                Ok(val) => Some(serde_json::to_string(&val).unwrap_or_else(|_| "null".into())),
                Err(e) => {
                    log::warn!(
                        "[plugin:{}] query_plugin({target}, {method}) error: {e}",
                        self.name
                    );
                    None
                }
            },
            Err(e) => {
                log::warn!(
                    "[plugin:{}] query_plugin({target}, {method}) failed: {e}",
                    self.name
                );
                None
            }
        }
    }

    fn register_service(&self, name: &str) {
        self.bus.register_service(&self.name, name);
    }

    fn get_config(&self, key: &str) -> Option<String> {
        let dir = conch_core::config::config_dir()
            .join("plugins")
            .join(&self.name);
        let path = dir.join(format!("{key}.json"));
        fs::read_to_string(&path).ok()
    }

    fn set_config(&self, key: &str, value: &str) {
        let dir = conch_core::config::config_dir()
            .join("plugins")
            .join(&self.name);
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(format!("{key}.json"));
        if let Err(e) = conch_core::config::atomic_write(&path, value.as_bytes()) {
            log::error!(
                "[plugin:{}] Failed to save config key '{key}' atomically: {e}",
                self.name
            );
        }
    }

    fn clipboard_set(&self, text: &str) {
        match arboard::Clipboard::new() {
            Ok(mut cb) => {
                if let Err(e) = cb.set_text(text) {
                    log::warn!("[{}] clipboard set failed: {e}", self.name);
                }
            }
            Err(e) => log::warn!("[{}] clipboard unavailable: {e}", self.name),
        }
    }

    fn clipboard_get(&self) -> Option<String> {
        match arboard::Clipboard::new() {
            Ok(mut cb) => cb.get_text().ok(),
            Err(e) => {
                log::warn!("[{}] clipboard unavailable: {e}", self.name);
                None
            }
        }
    }

    fn get_theme(&self) -> Option<String> {
        let tauri_state = self.app_handle.state::<crate::TauriState>();
        let cfg = tauri_state.config.read();
        let colors = crate::theme::resolve_theme_colors(&cfg);
        let appearance_mode = format!("{:?}", cfg.colors.appearance_mode).to_lowercase();
        let dark_mode = !matches!(
            cfg.colors.appearance_mode,
            conch_core::config::AppearanceMode::Light
        );
        Some(
            serde_json::json!({
                "name": cfg.colors.theme,
                "appearance_mode": appearance_mode,
                "dark_mode": dark_mode,
                "colors": colors,
            })
            .to_string(),
        )
    }

    fn get_active_session(&self) -> Option<String> {
        let tauri_state = self.app_handle.state::<crate::TauriState>();
        let (window_label, pane_id) = self.active_window_and_pane()?;

        let key = crate::pty::session_key(&window_label, pane_id);

        let remote_state = self
            .app_handle
            .state::<Arc<Mutex<crate::remote::RemoteState>>>();
        if let Some(remote) = remote_state.lock().sessions.get(&key) {
            return Some(
                serde_json::json!({
                    "window_label": window_label,
                    "pane_id": pane_id,
                    "key": key,
                    "type": "ssh",
                    "host": remote.host,
                    "user": remote.user,
                    "port": remote.port,
                })
                .to_string(),
            );
        }

        if tauri_state.ptys.lock().contains_key(&key) {
            return Some(
                serde_json::json!({
                    "window_label": window_label,
                    "pane_id": pane_id,
                    "key": key,
                    "type": "local",
                })
                .to_string(),
            );
        }

        Some(
            serde_json::json!({
                "window_label": window_label,
                "pane_id": pane_id,
                "key": key,
                "type": "unknown",
            })
            .to_string(),
        )
    }

    fn exec_active_session(&self, command: &str) -> Option<String> {
        let tauri_state = self.app_handle.state::<crate::TauriState>();
        let (window_label, pane_id) = self.active_window_and_pane()?;
        let key = crate::pty::session_key(&window_label, pane_id);

        let remote_state = self
            .app_handle
            .state::<Arc<Mutex<crate::remote::RemoteState>>>();

        let ssh_handle = {
            let state = remote_state.lock();
            if let Some(session) = state.sessions.get(&key) {
                state
                    .connections
                    .get(&session.connection_id)
                    .map(|conn| Arc::clone(&conn.ssh_handle))
            } else {
                None
            }
        };

        if let Some(ssh_handle) = ssh_handle {
            let result =
                tauri::async_runtime::block_on(conch_remote::ssh::exec(&ssh_handle, command));
            return Some(match result {
                Ok((stdout, stderr, exit_code)) => serde_json::json!({
                    "status": "ok",
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code as i64,
                })
                .to_string(),
                Err(e) => serde_json::json!({
                    "status": "error",
                    "stdout": "",
                    "stderr": e.to_string(),
                    "exit_code": -1,
                })
                .to_string(),
            });
        }

        if tauri_state.ptys.lock().contains_key(&key) {
            return Some(
                match std::process::Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .output()
                {
                    Ok(output) => serde_json::json!({
                        "status": "ok",
                        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                        "exit_code": output.status.code().unwrap_or(-1),
                    })
                    .to_string(),
                    Err(e) => serde_json::json!({
                        "status": "error",
                        "stdout": "",
                        "stderr": e.to_string(),
                        "exit_code": -1,
                    })
                    .to_string(),
                },
            );
        }

        Some(
            serde_json::json!({
                "status": "error",
                "stdout": "",
                "stderr": "no active session",
                "exit_code": -1,
            })
            .to_string(),
        )
    }

    fn register_menu_item(&self, menu: &str, label: &str, action: &str, keybind: Option<&str>) {
        self.register_menu_item_as(&self.name, menu, label, action, keybind);
    }

    fn register_menu_item_as(
        &self,
        plugin_name: &str,
        menu: &str,
        label: &str,
        action: &str,
        keybind: Option<&str>,
    ) {
        let item = PluginMenuItem {
            plugin: plugin_name.to_string(),
            menu: menu.to_string(),
            label: label.to_string(),
            action: action.to_string(),
            keybind: keybind.map(String::from),
        };
        self.menu_items.write().push(item.clone());

        // Also emit to frontend for immediate update.
        let _ = self.app_handle.emit("plugin-menu-item", &item);
    }

    fn show_form(&self, json: &str) -> Option<String> {
        let prompt_id = format!("{}\0{}", self.name, uuid::Uuid::new_v4());
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_dialogs
            .lock()
            .forms
            .insert(prompt_id.clone(), tx);
        let _ = self.app_handle.emit(
            "plugin-form-dialog",
            serde_json::json!({
                "prompt_id": prompt_id,
                "json": json,
            }),
        );
        // Block the plugin thread until the frontend responds.
        rx.blocking_recv().unwrap_or(None)
    }

    fn show_confirm(&self, msg: &str) -> bool {
        let prompt_id = format!("{}\0{}", self.name, uuid::Uuid::new_v4());
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_dialogs
            .lock()
            .confirms
            .insert(prompt_id.clone(), tx);
        let _ = self.app_handle.emit(
            "plugin-confirm-dialog",
            serde_json::json!({
                "prompt_id": prompt_id,
                "message": msg,
            }),
        );
        rx.blocking_recv().unwrap_or(false)
    }

    fn show_prompt(&self, msg: &str, default_value: &str) -> Option<String> {
        let prompt_id = format!("{}\0{}", self.name, uuid::Uuid::new_v4());
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_dialogs
            .lock()
            .prompts
            .insert(prompt_id.clone(), tx);
        let _ = self.app_handle.emit(
            "plugin-prompt-dialog",
            serde_json::json!({
                "prompt_id": prompt_id,
                "message": msg,
                "default_value": default_value,
            }),
        );
        rx.blocking_recv().unwrap_or(None)
    }

    fn show_alert(&self, title: &str, msg: &str) {
        // Use the toast notification system.
        let _ = self.app_handle.emit(
            "plugin-notification",
            PluginNotification {
                plugin: self.name.clone(),
                json: serde_json::json!({
                    "title": title,
                    "body": msg,
                    "level": "info",
                    "duration_ms": 4000,
                })
                .to_string(),
            },
        );
    }

    fn show_error(&self, title: &str, msg: &str) {
        let _ = self.app_handle.emit(
            "plugin-notification",
            PluginNotification {
                plugin: self.name.clone(),
                json: serde_json::json!({
                    "title": title,
                    "body": msg,
                    "level": "error",
                    "duration_ms": 6000,
                })
                .to_string(),
            },
        );
    }

    fn show_context_menu(&self, _json: &str) -> Option<String> {
        None
    }

    fn write_to_pty(&self, data: &[u8]) {
        // Emit to frontend — it will route to the active tab's PTY.
        let text = String::from_utf8_lossy(data).into_owned();
        let _ = self.app_handle.emit("plugin-write-pty", text);
    }

    fn new_tab(&self, command: Option<&str>, _plain: bool) {
        let _ = self.app_handle.emit(
            "plugin-new-tab",
            serde_json::json!({
                "command": command,
            }),
        );
    }

    fn open_session(&self, _meta_json: &str) -> u64 {
        // Session management is handled natively by Tauri's SSH module.
        0
    }

    fn close_session(&self, _handle: u64) {}
    fn set_session_status(&self, _handle: u64, _status: u8, _detail: Option<&str>) {}
    fn session_prompt(
        &self,
        _handle: u64,
        _prompt_type: u8,
        _msg: &str,
        _detail: Option<&str>,
    ) -> Option<String> {
        None
    }
}
