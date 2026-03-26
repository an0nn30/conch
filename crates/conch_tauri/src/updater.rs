//! In-app update checking, downloading, and installation.
//!
//! Wraps `tauri-plugin-updater` with Tauri commands that the frontend can invoke.
//! The non-serializable [`tauri_plugin_updater::Update`] is stashed in managed
//! state so that `check_for_update` and `install_update` can be separate calls.

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;

// ---------------------------------------------------------------------------
// Managed state
// ---------------------------------------------------------------------------

/// Holds a pending [`tauri_plugin_updater::Update`] between the check and
/// install steps. The `Update` struct is not serializable, so we keep it here
/// and hand the frontend only the lightweight [`UpdateInfo`].
pub(crate) struct PendingUpdate(pub(crate) Mutex<Option<tauri_plugin_updater::Update>>);

impl PendingUpdate {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(None))
    }
}

// ---------------------------------------------------------------------------
// Serializable DTOs
// ---------------------------------------------------------------------------

/// Lightweight summary of an available update, safe to send to the frontend.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct UpdateInfo {
    pub version: String,
    pub body: Option<String>,
}

/// Progress payload emitted as `update-progress` events during download.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Check the configured update endpoint for a new release.
///
/// On success returns `Ok(Some(UpdateInfo))` when an update is available, or
/// `Ok(None)` when the app is already up-to-date. The raw [`Update`] object is
/// stored in [`PendingUpdate`] for a subsequent `install_update` call.
#[tauri::command]
pub(crate) async fn check_for_update<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let response = updater.check().await.map_err(|e| e.to_string())?;

    match response {
        Some(update) => {
            let info = UpdateInfo {
                version: update.version.clone(),
                body: update.body.clone(),
            };
            // Stash the non-serializable Update for install_update.
            let state = app.state::<PendingUpdate>();
            *state.0.lock() = Some(update);
            log::info!("Update available: v{}", info.version);
            Ok(Some(info))
        }
        None => {
            log::info!("No update available");
            Ok(None)
        }
    }
}

/// Download and install the pending update.
///
/// Emits `update-progress` events with [`DownloadProgress`] payloads so the
/// frontend can show a progress indicator. Does **not** restart the app —
/// call `restart_app` separately after informing the user.
#[tauri::command]
pub(crate) async fn install_update<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let update = {
        let state = app.state::<PendingUpdate>();
        state.0.lock().take()
    };

    let update = update.ok_or_else(|| "No pending update to install".to_string())?;

    let app_handle = app.clone();
    let mut downloaded: u64 = 0;

    update
        .download_and_install(
            |chunk_size, content_length| {
                downloaded += chunk_size as u64;
                let progress = DownloadProgress {
                    downloaded,
                    total: content_length,
                };
                let _ = app_handle.emit("update-progress", &progress);
            },
            || {
                log::info!("Update download complete, installing...");
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    log::info!("Update installed successfully");
    Ok(())
}

/// Restart the application. Uses `AppHandle::request_restart` which is made
/// reliable by `tauri-plugin-process`.
#[tauri::command]
pub(crate) fn restart_app<R: Runtime>(app: AppHandle<R>) {
    log::info!("Restarting application for update...");
    app.request_restart();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_info_serializes() {
        let info = UpdateInfo {
            version: "1.2.3".into(),
            body: Some("Bug fixes and improvements".into()),
        };
        let json = serde_json::to_value(&info).expect("serialize UpdateInfo");
        assert_eq!(json["version"], "1.2.3");
        assert_eq!(json["body"], "Bug fixes and improvements");
    }

    #[test]
    fn update_info_serializes_with_no_body() {
        let info = UpdateInfo {
            version: "2.0.0".into(),
            body: None,
        };
        let json = serde_json::to_value(&info).expect("serialize UpdateInfo");
        assert_eq!(json["version"], "2.0.0");
        assert!(json["body"].is_null());
    }

    #[test]
    fn download_progress_serializes() {
        let progress = DownloadProgress {
            downloaded: 1024,
            total: Some(4096),
        };
        let json = serde_json::to_value(&progress).expect("serialize DownloadProgress");
        assert_eq!(json["downloaded"], 1024);
        assert_eq!(json["total"], 4096);
    }

    #[test]
    fn download_progress_serializes_unknown_total() {
        let progress = DownloadProgress {
            downloaded: 512,
            total: None,
        };
        let json = serde_json::to_value(&progress).expect("serialize DownloadProgress");
        assert_eq!(json["downloaded"], 512);
        assert!(json["total"].is_null());
    }

    #[test]
    fn pending_update_starts_empty() {
        let pending = PendingUpdate::new();
        assert!(pending.0.lock().is_none());
    }
}
