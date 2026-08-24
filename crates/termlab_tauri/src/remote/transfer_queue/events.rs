use async_trait::async_trait;
use serde::Serialize;
use termlab_remote::transfer::{TransferKind, TransferProgress, TransferStatus};
use ts_rs::TS;

use super::model::{TransferDirection, TransferJob, TransferJobState, TransferQueueSummary};

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueEventPayload {
    #[ts(as = "f64")]
    pub revision: u64,
    pub job: TransferJob,
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
    use serde_json::json;

    use super::QueueSummaryPayload;
    use crate::remote::transfer_queue::model::TransferQueueSummary;

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
}
