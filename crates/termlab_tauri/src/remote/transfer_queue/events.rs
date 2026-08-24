use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use tauri::Emitter;
use termlab_remote::transfer::SourceFingerprint;
use termlab_remote::transfer::{TransferKind, TransferProgress, TransferStatus};
use tokio::sync::oneshot;
use ts_rs::TS;
use uuid::Uuid;

use super::model::{
    CommitPhase, ManagedArtifacts, QueueSettings, TransferDirection, TransferJob, TransferJobState,
    TransferQueueSummary,
};
use super::runner::ProgressSlot;

pub const TRANSFER_JOB_UPDATED_EVENT: &str = "transfer-job-updated";
pub const TRANSFER_QUEUE_SUMMARY_EVENT: &str = "transfer-queue-summary";
pub const TRANSFER_PROGRESS_EVENT: &str = "transfer-progress";

#[derive(Debug)]
pub enum RunnerEvent {
    Checking {
        job_id: Uuid,
        lease_id: Uuid,
        ack: oneshot::Sender<Result<(), String>>,
    },
    Fingerprinted {
        job_id: Uuid,
        lease_id: Uuid,
        fingerprint: SourceFingerprint,
        total_bytes: u64,
        artifacts: ManagedArtifacts,
        ack: oneshot::Sender<Result<(), String>>,
    },
    DurableCheckpoint {
        job_id: Uuid,
        lease_id: Uuid,
        bytes: u64,
        ack: oneshot::Sender<Result<(), String>>,
    },
    CommitPhase {
        job_id: Uuid,
        lease_id: Uuid,
        phase: CommitPhase,
        ack: oneshot::Sender<Result<(), String>>,
    },
    ProgressReady {
        job_id: Uuid,
        lease_id: Uuid,
        slot: Arc<ProgressSlot>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueEventPayload {
    #[ts(as = "f64")]
    pub revision: u64,
    pub upserts: Vec<TransferJob>,
    pub removed_ids: Vec<Uuid>,
    pub queue_paused: bool,
    pub settings: QueueSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueSummaryPayload {
    #[ts(as = "f64")]
    pub revision: u64,
    pub summary: TransferQueueSummary,
}

#[async_trait]
pub trait TransferEventSink: Send + Sync {
    async fn job_updated(&self, payload: QueueEventPayload);
    async fn queue_summary(&self, payload: QueueSummaryPayload);
    async fn legacy_progress(&self, payload: TransferProgress);
}

pub struct TauriTransferEventSink<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriTransferEventSink<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }

    fn emit<T: Serialize + Clone>(&self, event: &str, payload: T) {
        if let Err(error) = self.app.emit(event, payload) {
            log::warn!("failed to emit {event}: {error}");
        }
    }
}

#[async_trait]
impl<R: tauri::Runtime> TransferEventSink for TauriTransferEventSink<R> {
    async fn job_updated(&self, payload: QueueEventPayload) {
        self.emit(TRANSFER_JOB_UPDATED_EVENT, payload);
    }

    async fn queue_summary(&self, payload: QueueSummaryPayload) {
        self.emit(TRANSFER_QUEUE_SUMMARY_EVENT, payload);
    }

    async fn legacy_progress(&self, payload: TransferProgress) {
        self.emit(TRANSFER_PROGRESS_EVENT, payload);
    }
}

pub fn legacy_progress_for(job: &TransferJob) -> TransferProgress {
    let status = match &job.state {
        TransferJobState::Queued
        | TransferJobState::Paused
        | TransferJobState::NeedsConnection { .. }
        | TransferJobState::NeedsAttention { .. }
        | TransferJobState::RetryWaiting { .. } => TransferStatus::Pending,
        TransferJobState::Connecting | TransferJobState::Checking | TransferJobState::Running => {
            TransferStatus::InProgress
        }
        TransferJobState::Completed { .. } => TransferStatus::Completed,
        TransferJobState::Failed { .. } => TransferStatus::Failed,
        TransferJobState::Cancelled { .. } => TransferStatus::Cancelled,
    };
    let error = match &job.state {
        TransferJobState::Failed { error } => Some(error.clone()),
        TransferJobState::Cancelled {
            cleanup_error: Some(error),
        } => Some(error.clone()),
        _ => None,
    };

    TransferProgress {
        transfer_id: job.id.to_string(),
        kind: match job.direction {
            TransferDirection::Upload => TransferKind::Upload,
            TransferDirection::Download => TransferKind::Download,
        },
        status,
        bytes_transferred: job.bytes_transferred,
        total_bytes: job.total_bytes,
        file_name: job.file_name.clone(),
        error,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use tauri::Listener;
    use uuid::Uuid;

    use super::{
        QueueEventPayload, QueueSummaryPayload, TRANSFER_JOB_UPDATED_EVENT,
        TRANSFER_PROGRESS_EVENT, TRANSFER_QUEUE_SUMMARY_EVENT, TauriTransferEventSink,
        TransferEventSink, legacy_progress_for,
    };
    use crate::remote::transfer_queue::model::{
        AttentionReason, CommitPhase, CompletionResult, ConflictPolicy, ManagedArtifacts,
        QueueSettings, TransferDirection, TransferEndpoint, TransferJob, TransferJobState,
        TransferOrigin, TransferPriority, TransferProtocol, TransferQueueSummary,
    };

    fn job(state: TransferJobState) -> TransferJob {
        TransferJob {
            id: Uuid::from_u128(0xabc),
            protocol: TransferProtocol::Sftp,
            direction: TransferDirection::Download,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: "server-1".into(),
                label: "Production".into(),
            },
            local_path: "/tmp/report.csv".into(),
            remote_path: "/srv/report.csv".into(),
            file_name: "report.csv".into(),
            batch_id: None,
            priority: TransferPriority::Normal,
            queue_order: 1,
            host_key: "configured:server-1".into(),
            destination_key: "configured:server-1:/tmp/report.csv".into(),
            state,
            source_fingerprint: None,
            durable_checkpoint: 2048,
            bytes_transferred: 3072,
            total_bytes: 4096,
            speed_bytes_per_second: 512,
            eta_seconds: Some(2),
            retry_attempt: 0,
            max_attempts: 3,
            conflict_policy: ConflictPolicy::Ask,
            artifacts: Some(ManagedArtifacts {
                partial_path: "/tmp/.report.partial".into(),
                backup_path: "/tmp/.report.backup".into(),
            }),
            commit_phase: CommitPhase::Prepared,
            created_at_ms: 1,
            updated_at_ms: 2,
            started_at_ms: Some(1),
            finished_at_ms: None,
        }
    }

    #[test]
    fn job_event_payload_is_a_complete_atomic_delta() {
        let removed = Uuid::from_u128(9);
        let payload = QueueEventPayload {
            revision: 17,
            upserts: Vec::new(),
            removed_ids: vec![removed],
            queue_paused: false,
            settings: QueueSettings {
                global_limit: 4,
                per_host_limit: 6,
            },
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "revision": 17,
                "upserts": [],
                "removedIds": [removed],
                "queuePaused": false,
                "settings": {
                    "globalLimit": 4,
                    "perHostLimit": 6
                }
            })
        );
    }

    #[test]
    fn summary_payload_carries_the_revision_frontends_use_for_gap_detection() {
        let payload = QueueSummaryPayload {
            revision: 17,
            summary: TransferQueueSummary {
                queued: 2,
                running: 1,
                paused: 0,
                attention: 0,
                failed: 0,
                active: 3,
                history: 4,
                queue_paused: false,
            },
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "revision": 17,
                "summary": {
                    "queued": 2,
                    "running": 1,
                    "paused": 0,
                    "attention": 0,
                    "failed": 0,
                    "active": 3,
                    "history": 4,
                    "queuePaused": false
                }
            })
        );
    }

    #[test]
    fn legacy_progress_keeps_uuid_direction_filename_and_bytes_for_all_terminal_mappings() {
        let cases = [
            (TransferJobState::Running, "in_progress"),
            (
                TransferJobState::Completed {
                    result: CompletionResult::Transferred,
                },
                "completed",
            ),
            (
                TransferJobState::Failed {
                    error: "disk full".into(),
                },
                "failed",
            ),
            (
                TransferJobState::Cancelled {
                    cleanup_error: None,
                },
                "cancelled",
            ),
        ];

        for (state, status) in cases {
            let payload = legacy_progress_for(&job(state));
            assert_eq!(
                serde_json::to_value(payload).unwrap(),
                json!({
                    "transfer_id": Uuid::from_u128(0xabc).to_string(),
                    "kind": "download",
                    "status": status,
                    "bytes_transferred": 3072,
                    "total_bytes": 4096,
                    "file_name": "report.csv",
                    "error": if status == "failed" { Some("disk full") } else { None },
                })
            );
        }
    }

    #[test]
    fn connection_and_attention_states_never_masquerade_as_legacy_terminal_success() {
        let states = [
            TransferJobState::NeedsConnection {
                message: "Reconnect".into(),
            },
            TransferJobState::NeedsAttention {
                reason: AttentionReason::MissingPartial,
            },
        ];

        for state in states {
            let payload = serde_json::to_value(legacy_progress_for(&job(state))).unwrap();
            assert_eq!(payload["status"], "pending");
        }
    }

    #[tokio::test]
    async fn tauri_sink_emits_the_exact_queue_and_legacy_event_names() {
        let app = tauri::test::mock_app();
        let received = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
        for event_name in [
            TRANSFER_JOB_UPDATED_EVENT,
            TRANSFER_QUEUE_SUMMARY_EVENT,
            TRANSFER_PROGRESS_EVENT,
        ] {
            let received = Arc::clone(&received);
            app.listen(event_name, move |event| {
                received.lock().unwrap().push((
                    event_name.to_string(),
                    serde_json::from_str(event.payload()).unwrap(),
                ));
            });
        }
        let sink = TauriTransferEventSink::new(app.handle().clone());
        let updated_job = job(TransferJobState::Running);
        let delta = QueueEventPayload {
            revision: 41,
            upserts: vec![updated_job.clone()],
            removed_ids: vec![Uuid::from_u128(0xdef)],
            queue_paused: false,
            settings: QueueSettings {
                global_limit: 4,
                per_host_limit: 2,
            },
        };
        let summary = QueueSummaryPayload {
            revision: 41,
            summary: TransferQueueSummary {
                queued: 3,
                running: 1,
                paused: 2,
                attention: 4,
                failed: 5,
                active: 10,
                history: 6,
                queue_paused: false,
            },
        };
        let legacy = legacy_progress_for(&updated_job);

        sink.job_updated(delta.clone()).await;
        sink.queue_summary(summary.clone()).await;
        sink.legacy_progress(legacy.clone()).await;

        assert_eq!(
            *received.lock().unwrap(),
            vec![
                (
                    "transfer-job-updated".into(),
                    serde_json::to_value(delta).unwrap()
                ),
                (
                    "transfer-queue-summary".into(),
                    serde_json::to_value(summary).unwrap()
                ),
                (
                    "transfer-progress".into(),
                    serde_json::to_value(legacy).unwrap()
                ),
            ]
        );
    }
}
