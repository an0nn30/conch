//! File transfer Tauri commands — download, upload, cancel.
//!
//! Both transfers resolve the calling window through `session_caller_label`
//! for the same reason every SFTP command does: a popped-out panel host (or a
//! chooser window) transfers on behalf of its PARENT, and sessions are keyed
//! under the parent's label. Using `window.label()` raw here meant a
//! popped-out panel's upload/download failed with "No SSH session for
//! {own-label}:{pane}".

use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Deserializer, de::Error as _};
use tauri::Manager;
use uuid::Uuid;

use super::RemoteState;
use super::sftp_commands::session_caller_label;
use super::transfer_queue::TransferQueueHandle;
use super::transfer_queue::model::{
    ConflictPolicy, ConflictResolution, NewTransferJob, QueueSettings, TransferDirection,
    TransferEndpoint, TransferOrigin, TransferPriority, TransferProtocol, TransferQueueSnapshot,
    build_destination_key, build_host_key,
};

#[derive(Clone)]
pub(super) struct TransferSessionSnapshot {
    server_entry_id: Option<String>,
    server_label: Option<String>,
    host: String,
    port: u16,
    user: String,
    proxy_command: Option<String>,
    proxy_jump: Option<String>,
}

/// Compatibility shape accepted only at the Tauri command boundary.
/// Durable jobs and events continue to use the canonical tagged enum.
#[derive(Debug)]
pub(crate) struct TransferOriginCommand(TransferOrigin);

#[derive(Deserialize)]
#[serde(untagged)]
enum TransferOriginWire {
    Name(String),
    Tagged(TransferOrigin),
}

impl<'de> Deserialize<'de> for TransferOriginCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match TransferOriginWire::deserialize(deserializer)? {
            TransferOriginWire::Name(name) if name == "filesPanel" => {
                Ok(Self(TransferOrigin::FilesPanel))
            }
            TransferOriginWire::Name(name) if name == "editor" => Ok(Self(TransferOrigin::Editor)),
            TransferOriginWire::Name(name) => Err(D::Error::custom(format!(
                "invalid transfer origin '{name}'; expected 'filesPanel', 'editor', or a tagged origin object"
            ))),
            TransferOriginWire::Tagged(origin) => Ok(Self(origin)),
        }
    }
}

impl TransferOriginCommand {
    fn into_origin(self) -> TransferOrigin {
        self.0
    }
}

pub(super) trait TransferSessionLookup {
    fn transfer_session(
        &self,
        window_label: &str,
        pane_id: u32,
    ) -> Result<TransferSessionSnapshot, String>;
}

impl TransferSessionLookup for RemoteState {
    fn transfer_session(
        &self,
        window_label: &str,
        pane_id: u32,
    ) -> Result<TransferSessionSnapshot, String> {
        let key = super::session_key(window_label, pane_id);
        let session = self
            .sessions
            .get(&key)
            .ok_or_else(|| format!("No SSH session for {key}"))?;
        let connection = self
            .connections
            .get(&session.connection_id)
            .ok_or_else(|| format!("No SSH connection for {}", session.connection_id))?;
        let server_entry_id = connection
            .server_entry_id
            .clone()
            .or_else(|| session.server_entry_id.clone());
        let server_label = server_entry_id.as_deref().and_then(|id| {
            self.config
                .find_server(id)
                .or_else(|| self.ssh_config_entries.iter().find(|entry| entry.id == id))
                .map(|entry| entry.label.clone())
        });

        Ok(TransferSessionSnapshot {
            server_entry_id,
            server_label,
            host: connection.host.clone(),
            port: connection.port,
            user: connection.user.clone(),
            proxy_command: connection.proxy_command.clone(),
            proxy_jump: connection.proxy_jump.clone(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_transfer_request(
    sessions: &impl TransferSessionLookup,
    caller_label: &str,
    parent_label: Option<&str>,
    pane_id: u32,
    id: Uuid,
    direction: TransferDirection,
    local_path: String,
    remote_path: String,
    origin: Option<TransferOrigin>,
    conflict_policy: Option<ConflictPolicy>,
    batch_id: Option<Uuid>,
) -> Result<NewTransferJob, String> {
    let window_label = parent_label.unwrap_or(caller_label);
    let session = sessions.transfer_session(window_label, pane_id)?;
    let endpoint = match session.server_entry_id {
        Some(server_entry_id) => TransferEndpoint::Configured {
            label: session
                .server_label
                .unwrap_or_else(|| format!("{}@{}:{}", session.user, session.host, session.port)),
            server_entry_id,
        },
        None => TransferEndpoint::AdHoc {
            host: session.host,
            port: session.port,
            user: session.user,
            proxy_command: session.proxy_command,
            proxy_jump: session.proxy_jump,
        },
    };
    let origin = origin.unwrap_or(TransferOrigin::FilesPanel);
    let priority = if origin == TransferOrigin::Editor {
        TransferPriority::Interactive
    } else {
        TransferPriority::Normal
    };
    let host_key = build_host_key(&endpoint);
    let destination_key = build_destination_key(&host_key, &direction, &local_path, &remote_path);
    let file_path = match direction {
        TransferDirection::Upload => &local_path,
        TransferDirection::Download => &remote_path,
    };
    let file_name = file_path
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(file_path)
        .to_string();

    Ok(NewTransferJob {
        id,
        protocol: TransferProtocol::Sftp,
        direction,
        origin,
        endpoint,
        local_path,
        remote_path,
        file_name,
        batch_id,
        priority,
        host_key,
        destination_key,
        conflict_policy: conflict_policy.unwrap_or(ConflictPolicy::Ask),
    })
}

async fn enqueue_transfer(
    queue: &TransferQueueHandle,
    request: NewTransferJob,
) -> Result<String, String> {
    let requested_id = request.id;
    let stored_id = queue.enqueue(request).await?;
    if stored_id != requested_id {
        return Err("transfer queue returned a different job id".into());
    }
    Ok(stored_id.to_string())
}

fn parse_transfer_id(transfer_id: &str) -> Result<Uuid, String> {
    Uuid::parse_str(transfer_id).map_err(|_| format!("Invalid transfer id '{transfer_id}'"))
}

async fn cancel_transfer(queue: &TransferQueueHandle, transfer_id: &str) -> bool {
    let Ok(id) = parse_transfer_id(transfer_id) else {
        return false;
    };
    queue.cancel(id).await.unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn transfer_download(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    queue: tauri::State<'_, TransferQueueHandle>,
    pane_id: u32,
    remote_path: String,
    local_path: String,
    origin: Option<TransferOriginCommand>,
    conflict_policy: Option<ConflictPolicy>,
) -> Result<String, String> {
    let id = Uuid::new_v4();
    let caller_label = window.label();
    let session_label = session_caller_label(&window);
    let parent_label = (session_label != caller_label).then_some(session_label.as_str());
    let request = {
        let state = remote.lock();
        build_transfer_request(
            &*state,
            caller_label,
            parent_label,
            pane_id,
            id,
            TransferDirection::Download,
            local_path,
            remote_path,
            origin.map(TransferOriginCommand::into_origin),
            conflict_policy,
            None,
        )?
    };
    enqueue_transfer(&queue, request).await
}

#[tauri::command]
pub(crate) async fn transfer_upload(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    queue: tauri::State<'_, TransferQueueHandle>,
    pane_id: u32,
    local_path: String,
    remote_path: String,
    origin: Option<TransferOriginCommand>,
    conflict_policy: Option<ConflictPolicy>,
) -> Result<String, String> {
    let id = Uuid::new_v4();
    let caller_label = window.label();
    let session_label = session_caller_label(&window);
    let parent_label = (session_label != caller_label).then_some(session_label.as_str());
    let request = {
        let state = remote.lock();
        build_transfer_request(
            &*state,
            caller_label,
            parent_label,
            pane_id,
            id,
            TransferDirection::Upload,
            local_path,
            remote_path,
            origin.map(TransferOriginCommand::into_origin),
            conflict_policy,
            None,
        )?
    };
    enqueue_transfer(&queue, request).await
}

/// Transfer a whole folder. Validates the source, creates the batch, spawns
/// the expansion task, and returns the batch id immediately — discovered
/// files are enqueued (and may start transferring) while the walk continues.
#[tauri::command]
pub(crate) async fn transfer_enqueue_recursive(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    queue: tauri::State<'_, TransferQueueHandle>,
    pane_id: u32,
    direction: TransferDirection,
    source_path: String,
    dest_path: String,
) -> Result<String, String> {
    let caller_label = window.label().to_string();
    let session_label = session_caller_label(&window);
    let parent_label = (session_label != caller_label).then_some(session_label);

    super::recursive_transfer::start_recursive_transfer(
        queue.inner().clone(),
        Arc::clone(remote.inner()),
        caller_label,
        parent_label,
        pane_id,
        direction,
        source_path,
        dest_path,
    )
    .await
}

#[tauri::command]
pub(crate) async fn transfer_cancel_batch(
    queue: tauri::State<'_, TransferQueueHandle>,
    batch_id: String,
) -> Result<(), String> {
    let id = Uuid::parse_str(&batch_id).map_err(|_| format!("Invalid batch id '{batch_id}'"))?;
    queue.cancel_batch(id).await
}

#[tauri::command]
pub(crate) async fn transfer_cancel(app: tauri::AppHandle, transfer_id: String) -> bool {
    let queue = app.state::<TransferQueueHandle>().inner().clone();
    cancel_transfer(&queue, &transfer_id).await
}

#[tauri::command]
pub(crate) fn transfer_queue_snapshot(
    queue: tauri::State<'_, TransferQueueHandle>,
) -> TransferQueueSnapshot {
    queue.snapshot()
}

#[tauri::command]
pub(crate) async fn transfer_pause(
    queue: tauri::State<'_, TransferQueueHandle>,
    transfer_id: String,
) -> Result<(), String> {
    queue.pause(parse_transfer_id(&transfer_id)?).await
}

#[tauri::command]
pub(crate) async fn transfer_resume(
    queue: tauri::State<'_, TransferQueueHandle>,
    transfer_id: String,
) -> Result<(), String> {
    queue.resume(parse_transfer_id(&transfer_id)?).await
}

#[tauri::command]
pub(crate) async fn transfer_pause_all(
    queue: tauri::State<'_, TransferQueueHandle>,
) -> Result<(), String> {
    queue.pause_all().await
}

#[tauri::command]
pub(crate) async fn transfer_resume_all(
    queue: tauri::State<'_, TransferQueueHandle>,
) -> Result<(), String> {
    queue.resume_all().await
}

#[tauri::command]
pub(crate) async fn transfer_retry(
    queue: tauri::State<'_, TransferQueueHandle>,
    transfer_id: String,
) -> Result<(), String> {
    queue.retry(parse_transfer_id(&transfer_id)?).await
}

#[tauri::command]
pub(crate) async fn transfer_resolve(
    queue: tauri::State<'_, TransferQueueHandle>,
    transfer_id: String,
    resolution: ConflictResolution,
) -> Result<(), String> {
    queue
        .resolve(parse_transfer_id(&transfer_id)?, resolution)
        .await
}

#[tauri::command]
pub(crate) async fn transfer_reorder(
    queue: tauri::State<'_, TransferQueueHandle>,
    transfer_id: String,
    before: Option<String>,
) -> Result<(), String> {
    let before = before.as_deref().map(parse_transfer_id).transpose()?;
    queue
        .reorder(parse_transfer_id(&transfer_id)?, before)
        .await
}

#[tauri::command]
pub(crate) async fn transfer_set_priority(
    queue: tauri::State<'_, TransferQueueHandle>,
    transfer_id: String,
    priority: TransferPriority,
) -> Result<(), String> {
    queue
        .set_priority(parse_transfer_id(&transfer_id)?, priority)
        .await
}

#[tauri::command]
pub(crate) async fn transfer_clear_completed(
    queue: tauri::State<'_, TransferQueueHandle>,
) -> Result<usize, String> {
    queue.clear_completed().await
}

#[tauri::command]
pub(crate) async fn transfer_update_settings(
    queue: tauri::State<'_, TransferQueueHandle>,
    settings: QueueSettings,
) -> Result<(), String> {
    queue.update_settings(settings).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use parking_lot::Mutex;
    use tauri::Manager;
    use uuid::Uuid;

    use super::*;
    use crate::panel_host::PanelHostRegistry;
    use crate::remote::transfer_queue::model::{
        ConflictPolicy, TransferDirection, TransferEndpoint, TransferOrigin, TransferPriority,
    };
    use crate::remote::transfer_queue::{
        QueueActor, QueueEventPayload, QueueSummaryPayload, TransferEventSink, store::TransferStore,
    };

    struct NoopEventSink;

    #[async_trait::async_trait]
    impl TransferEventSink for NoopEventSink {
        async fn job_updated(&self, _payload: QueueEventPayload) {}

        async fn queue_summary(&self, _payload: QueueSummaryPayload) {}

        async fn legacy_progress(&self, _payload: termlab_remote::transfer::TransferProgress) {}
    }

    #[derive(Default)]
    struct FakeSessionLookup {
        sessions: HashMap<(String, u32), TransferSessionSnapshot>,
    }

    impl FakeSessionLookup {
        fn with_session(
            mut self,
            window_label: &str,
            pane_id: u32,
            session: TransferSessionSnapshot,
        ) -> Self {
            self.sessions
                .insert((window_label.to_string(), pane_id), session);
            self
        }
    }

    impl TransferSessionLookup for FakeSessionLookup {
        fn transfer_session(
            &self,
            window_label: &str,
            pane_id: u32,
        ) -> Result<TransferSessionSnapshot, String> {
            self.sessions
                .get(&(window_label.to_string(), pane_id))
                .cloned()
                .ok_or_else(|| format!("No SSH session for {window_label}:{pane_id}"))
        }
    }

    fn configured_session() -> TransferSessionSnapshot {
        TransferSessionSnapshot {
            server_entry_id: Some("server-prod".into()),
            server_label: Some("Production".into()),
            host: "prod.example.com".into(),
            port: 22,
            user: "deploy".into(),
            proxy_command: None,
            proxy_jump: None,
        }
    }

    fn ad_hoc_session() -> TransferSessionSnapshot {
        TransferSessionSnapshot {
            server_entry_id: None,
            server_label: None,
            host: "shell.example.com".into(),
            port: 2202,
            user: "dustin".into(),
            proxy_command: Some("ssh -W %h:%p edge".into()),
            proxy_jump: Some("edge.example.com".into()),
        }
    }

    #[test]
    fn command_origin_accepts_approved_string_and_legacy_tagged_json_shapes() {
        let files: TransferOriginCommand = serde_json::from_str(r#""filesPanel""#).unwrap();
        let editor: TransferOriginCommand = serde_json::from_str(r#""editor""#).unwrap();
        let tagged: TransferOriginCommand =
            serde_json::from_str(r#"{"kind":"filesPanel"}"#).unwrap();
        let other: TransferOriginCommand =
            serde_json::from_str(r#"{"kind":"other","name":"commandPalette"}"#).unwrap();

        assert_eq!(files.into_origin(), TransferOrigin::FilesPanel);
        assert_eq!(editor.into_origin(), TransferOrigin::Editor);
        assert_eq!(tagged.into_origin(), TransferOrigin::FilesPanel);
        assert_eq!(
            other.into_origin(),
            TransferOrigin::Other {
                name: "commandPalette".into(),
            }
        );
    }

    #[test]
    fn command_origin_rejects_invalid_strings_with_a_clear_error() {
        for invalid in [r#""files_panel""#, r#""""#, r#""other""#] {
            let error = serde_json::from_str::<TransferOriginCommand>(invalid).unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("filesPanel") && message.contains("editor"),
                "{message}"
            );
        }
    }

    #[test]
    fn configured_detached_session_builds_a_canonical_normal_upload_request() {
        let id = Uuid::from_u128(0x101);
        let lookup =
            FakeSessionLookup::default().with_session("main", 1_000_000, configured_session());

        let request = build_transfer_request(
            &lookup,
            "main",
            None,
            1_000_000,
            id,
            TransferDirection::Upload,
            "/tmp/report.csv".into(),
            "/srv/releases/./daily/../report.csv".into(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(request.id, id);
        assert_eq!(
            request.endpoint,
            TransferEndpoint::Configured {
                server_entry_id: "server-prod".into(),
                label: "Production".into(),
            }
        );
        assert_eq!(request.origin, TransferOrigin::FilesPanel);
        assert_eq!(request.priority, TransferPriority::Normal);
        assert_eq!(request.conflict_policy, ConflictPolicy::Ask);
        assert_eq!(request.host_key, "configured:server-prod");
        assert_eq!(
            request.destination_key,
            "configured:server-prod:/srv/releases/report.csv"
        );
        assert_eq!(request.file_name, "report.csv");
    }

    #[test]
    fn terminal_ad_hoc_session_snapshots_proxy_and_local_download_destination() {
        let lookup = FakeSessionLookup::default().with_session("main", 7, ad_hoc_session());

        let request = build_transfer_request(
            &lookup,
            "main",
            None,
            7,
            Uuid::from_u128(0x102),
            TransferDirection::Download,
            "/tmp/downloads/./archive/../report.csv".into(),
            "/srv/report.csv".into(),
            Some(TransferOrigin::Other {
                name: "commandPalette".into(),
            }),
            Some(ConflictPolicy::Overwrite),
            None,
        )
        .unwrap();

        assert_eq!(
            request.endpoint,
            TransferEndpoint::AdHoc {
                host: "shell.example.com".into(),
                port: 2202,
                user: "dustin".into(),
                proxy_command: Some("ssh -W %h:%p edge".into()),
                proxy_jump: Some("edge.example.com".into()),
            }
        );
        assert_eq!(request.host_key, "adhoc:dustin@shell.example.com:2202");
        assert_eq!(request.destination_key, "local:/tmp/downloads/report.csv");
        assert_eq!(request.conflict_policy, ConflictPolicy::Overwrite);
    }

    #[test]
    fn panel_host_caller_uses_its_parent_session_label() {
        let app = tauri::test::mock_app();
        app.manage(Mutex::new(PanelHostRegistry::default()));
        let host_label = {
            let registry = app.state::<Mutex<PanelHostRegistry>>();
            registry
                .lock()
                .open("window-2".into(), "files".into(), "Files".into(), vec![])
                .1
                .window_label
                .clone()
        };
        let resolved = crate::window_registry_resolver::effective_session_window_label(
            app.handle(),
            &host_label,
        );
        let lookup = FakeSessionLookup::default().with_session("window-2", 3, configured_session());

        let request = build_transfer_request(
            &lookup,
            &host_label,
            Some(&resolved),
            3,
            Uuid::from_u128(0x103),
            TransferDirection::Upload,
            "/tmp/panel.txt".into(),
            "/srv/panel.txt".into(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(resolved, "window-2");
        assert_eq!(
            request.endpoint,
            TransferEndpoint::Configured {
                server_entry_id: "server-prod".into(),
                label: "Production".into(),
            }
        );
    }

    #[test]
    fn expansion_members_carry_their_batch_id_while_single_transfers_do_not() {
        let lookup = FakeSessionLookup::default().with_session("main", 5, configured_session());
        let batch_id = Uuid::from_u128(0x1_000);

        let member = build_transfer_request(
            &lookup,
            "main",
            None,
            5,
            Uuid::from_u128(0x108),
            TransferDirection::Upload,
            "/tmp/tree/a/one.txt".into(),
            "/srv/tree/a/one.txt".into(),
            None,
            None,
            Some(batch_id),
        )
        .unwrap();
        let single = build_transfer_request(
            &lookup,
            "main",
            None,
            5,
            Uuid::from_u128(0x109),
            TransferDirection::Upload,
            "/tmp/one.txt".into(),
            "/srv/one.txt".into(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(member.batch_id, Some(batch_id));
        assert_eq!(member.file_name, "one.txt");
        assert_eq!(single.batch_id, None);
    }

    #[test]
    fn missing_session_is_reported_without_building_a_request() {
        let error = build_transfer_request(
            &FakeSessionLookup::default(),
            "main",
            None,
            4,
            Uuid::from_u128(0x104),
            TransferDirection::Upload,
            "/tmp/missing.txt".into(),
            "/srv/missing.txt".into(),
            None,
            None,
            None,
        )
        .unwrap_err();

        assert_eq!(error, "No SSH session for main:4");
    }

    #[test]
    fn editor_origin_is_interactive_while_omitted_origin_is_files_panel_normal() {
        let lookup = FakeSessionLookup::default().with_session("main", 9, configured_session());
        let editor = build_transfer_request(
            &lookup,
            "main",
            None,
            9,
            Uuid::from_u128(0x105),
            TransferDirection::Download,
            "/tmp/editor.txt".into(),
            "/srv/editor.txt".into(),
            Some(TransferOrigin::Editor),
            None,
            None,
        )
        .unwrap();
        let legacy = build_transfer_request(
            &lookup,
            "main",
            None,
            9,
            Uuid::from_u128(0x106),
            TransferDirection::Download,
            "/tmp/legacy.txt".into(),
            "/srv/legacy.txt".into(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(editor.origin, TransferOrigin::Editor);
        assert_eq!(editor.priority, TransferPriority::Interactive);
        assert_eq!(legacy.origin, TransferOrigin::FilesPanel);
        assert_eq!(legacy.priority, TransferPriority::Normal);
    }

    #[tokio::test]
    async fn legacy_adapter_enqueues_and_returns_the_same_uuid() {
        let directory = tempfile::tempdir().unwrap();
        let queue = QueueActor::spawn(
            TransferStore::new(directory.path().join("transfers.json")),
            Arc::new(NoopEventSink),
        )
        .unwrap();
        let id = Uuid::from_u128(0x107);
        let lookup = FakeSessionLookup::default().with_session("main", 9, configured_session());
        let request = build_transfer_request(
            &lookup,
            "main",
            None,
            9,
            id,
            TransferDirection::Upload,
            "/tmp/queued.txt".into(),
            "/srv/queued.txt".into(),
            None,
            None,
            None,
        )
        .unwrap();

        let returned = enqueue_transfer(&queue, request).await.unwrap();

        assert_eq!(returned, id.to_string());
        assert_eq!(queue.snapshot().jobs[0].id, id);
    }

    #[tokio::test]
    async fn legacy_cancel_delegates_known_jobs_and_returns_false_for_unknown_ids() {
        let directory = tempfile::tempdir().unwrap();
        let queue = QueueActor::spawn(
            TransferStore::new(directory.path().join("transfers.json")),
            Arc::new(NoopEventSink),
        )
        .unwrap();
        let id = Uuid::from_u128(0x108);
        let lookup = FakeSessionLookup::default().with_session("main", 9, configured_session());
        let request = build_transfer_request(
            &lookup,
            "main",
            None,
            9,
            id,
            TransferDirection::Download,
            "/tmp/cancelled.txt".into(),
            "/srv/cancelled.txt".into(),
            None,
            None,
            None,
        )
        .unwrap();
        queue.enqueue(request).await.unwrap();
        queue.pause(id).await.unwrap();

        assert!(!cancel_transfer(&queue, "not-a-uuid").await);
        assert!(!cancel_transfer(&queue, &Uuid::from_u128(0xffff).to_string()).await);
        assert!(cancel_transfer(&queue, &id.to_string()).await);
        assert!(matches!(
            queue.snapshot().jobs[0].state,
            crate::remote::transfer_queue::model::TransferJobState::Cancelled {
                cleanup_error: None
            }
        ));
    }
}
