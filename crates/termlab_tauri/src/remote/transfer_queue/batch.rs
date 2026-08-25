use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::model::{TransferDirection, TransferJob, TransferJobState};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum BatchExpansion {
    Running,
    Complete,
    Interrupted { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BatchInfo {
    pub id: Uuid,
    pub name: String,
    pub direction: TransferDirection,
    pub expansion: BatchExpansion,
    #[ts(as = "f64")]
    pub discovered_files: u64,
    #[ts(as = "f64")]
    pub discovered_bytes: u64,
    /// Symlinks (and other skipped entries) recorded during expansion.
    #[serde(default)]
    pub skipped: Vec<String>,
    #[ts(as = "f64")]
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BatchAggregate {
    pub info: BatchInfo,
    #[ts(as = "f64")]
    pub files_done: u64,
    #[ts(as = "f64")]
    pub bytes_done: u64,
    #[ts(as = "Option<f64>")]
    pub speed_bytes_per_second: Option<u64>,
    #[ts(as = "Option<f64>")]
    pub eta_seconds: Option<u64>,
}

/// Roll up per-batch progress from the flat job list. Aggregates are derived
/// on read, never persisted, so they always reflect the current job states.
pub fn derive_batch_aggregates(
    batches: &BTreeMap<Uuid, BatchInfo>,
    jobs: &[TransferJob],
) -> Vec<BatchAggregate> {
    let mut aggregates: Vec<BatchAggregate> = batches
        .values()
        .map(|info| {
            let members = jobs.iter().filter(|job| job.batch_id == Some(info.id));

            let mut files_done = 0u64;
            let mut bytes_done = 0u64;
            let mut speed_sum = 0u64;

            for job in members {
                if matches!(job.state, TransferJobState::Completed { .. }) {
                    files_done += 1;
                }

                bytes_done += if job.total_bytes > 0 {
                    job.bytes_transferred.min(job.total_bytes)
                } else {
                    job.bytes_transferred
                };

                if matches!(job.state, TransferJobState::Running) {
                    // 0 is the persisted "unknown speed" sentinel (see
                    // model.rs), so it contributes nothing to the sum.
                    speed_sum += job.speed_bytes_per_second;
                }
            }

            let speed = (speed_sum > 0).then_some(speed_sum);
            let eta = speed.map(|speed| info.discovered_bytes.saturating_sub(bytes_done) / speed);

            BatchAggregate {
                info: info.clone(),
                files_done,
                bytes_done,
                speed_bytes_per_second: speed,
                eta_seconds: eta,
            }
        })
        .collect();

    aggregates.sort_by(|a, b| {
        a.info
            .created_at_ms
            .cmp(&b.info.created_at_ms)
            .then_with(|| a.info.id.cmp(&b.info.id))
    });

    aggregates
}

/// Drop batches with no remaining member jobs (active or history) so the
/// batch map does not grow without bound as jobs age out of history.
pub fn compact_batches(batches: &mut BTreeMap<Uuid, BatchInfo>, jobs: &[TransferJob]) {
    batches.retain(|id, _| jobs.iter().any(|job| job.batch_id == Some(*id)));
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::super::model::{
        CommitPhase, CompletionResult, ConflictPolicy, TransferDirection, TransferEndpoint,
        TransferJob, TransferJobState, TransferOrigin, TransferPriority, TransferProtocol,
        TransferQueueDocument,
    };
    use super::{BatchExpansion, BatchInfo, compact_batches, derive_batch_aggregates};
    use std::collections::BTreeMap;

    fn job(
        batch_id: Option<Uuid>,
        state: TransferJobState,
        bytes_transferred: u64,
        total_bytes: u64,
        speed_bytes_per_second: u64,
    ) -> TransferJob {
        TransferJob {
            id: Uuid::new_v4(),
            protocol: TransferProtocol::Sftp,
            direction: TransferDirection::Upload,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: "server-1".into(),
                label: "Production".into(),
            },
            local_path: "/tmp/report.csv".into(),
            remote_path: "/srv/report.csv".into(),
            file_name: "report.csv".into(),
            batch_id,
            priority: TransferPriority::Normal,
            queue_order: 1,
            host_key: "configured:server-1".into(),
            destination_key: "configured:server-1:/srv/report.csv".into(),
            state,
            source_fingerprint: None,
            durable_checkpoint: 0,
            bytes_transferred,
            total_bytes,
            speed_bytes_per_second,
            eta_seconds: None,
            retry_attempt: 0,
            max_attempts: 3,
            conflict_policy: ConflictPolicy::Ask,
            artifacts: None,
            commit_phase: CommitPhase::None,
            commit_backup_expected: None,
            created_at_ms: 10,
            updated_at_ms: 10,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }

    fn batch_info(
        id: Uuid,
        discovered_files: u64,
        discovered_bytes: u64,
        created_at_ms: u64,
    ) -> BatchInfo {
        BatchInfo {
            id,
            name: "batch".into(),
            direction: TransferDirection::Upload,
            expansion: BatchExpansion::Complete,
            discovered_files,
            discovered_bytes,
            skipped: Vec::new(),
            created_at_ms,
        }
    }

    #[test]
    fn derive_sums_done_files_bytes_and_speed() {
        let batch_id = Uuid::from_u128(1);
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, batch_info(batch_id, 3, 400, 10));

        let jobs = vec![
            job(
                Some(batch_id),
                TransferJobState::Completed {
                    result: CompletionResult::Transferred,
                },
                100,
                100,
                0,
            ),
            job(Some(batch_id), TransferJobState::Running, 40, 100, 50),
            job(Some(batch_id), TransferJobState::Queued, 0, 200, 0),
        ];

        let aggregates = derive_batch_aggregates(&batches, &jobs);

        assert_eq!(aggregates.len(), 1);
        let aggregate = &aggregates[0];
        assert_eq!(aggregate.files_done, 1);
        assert_eq!(aggregate.bytes_done, 140);
        assert_eq!(aggregate.speed_bytes_per_second, Some(50));
        assert_eq!(aggregate.eta_seconds, Some(5));
    }

    #[test]
    fn derive_speed_none_when_no_active_member() {
        let batch_id = Uuid::from_u128(1);
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, batch_info(batch_id, 2, 200, 10));

        let jobs = vec![
            job(
                Some(batch_id),
                TransferJobState::Completed {
                    result: CompletionResult::Transferred,
                },
                100,
                100,
                0,
            ),
            job(Some(batch_id), TransferJobState::Queued, 0, 100, 0),
        ];

        let aggregates = derive_batch_aggregates(&batches, &jobs);

        assert_eq!(aggregates.len(), 1);
        let aggregate = &aggregates[0];
        assert_eq!(aggregate.speed_bytes_per_second, None);
        assert_eq!(aggregate.eta_seconds, None);
    }

    #[test]
    fn derive_ignores_jobs_of_other_batches_and_batchless() {
        let batch_id = Uuid::from_u128(1);
        let other_batch_id = Uuid::from_u128(2);
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, batch_info(batch_id, 1, 100, 10));

        let jobs = vec![
            job(
                Some(batch_id),
                TransferJobState::Completed {
                    result: CompletionResult::Transferred,
                },
                100,
                100,
                0,
            ),
            job(
                Some(other_batch_id),
                TransferJobState::Completed {
                    result: CompletionResult::Transferred,
                },
                100,
                100,
                0,
            ),
            job(None, TransferJobState::Running, 40, 100, 50),
        ];

        let aggregates = derive_batch_aggregates(&batches, &jobs);

        assert_eq!(aggregates.len(), 1);
        let aggregate = &aggregates[0];
        assert_eq!(aggregate.files_done, 1);
        assert_eq!(aggregate.bytes_done, 100);
        assert_eq!(aggregate.speed_bytes_per_second, None);
    }

    #[test]
    fn derive_orders_by_created_then_id() {
        let earlier_id = Uuid::from_u128(20);
        let later_id = Uuid::from_u128(10);
        let mut batches = BTreeMap::new();
        batches.insert(later_id, batch_info(later_id, 1, 100, 20));
        batches.insert(earlier_id, batch_info(earlier_id, 1, 100, 10));

        let jobs = vec![
            job(Some(earlier_id), TransferJobState::Queued, 0, 100, 0),
            job(Some(later_id), TransferJobState::Queued, 0, 100, 0),
        ];

        let aggregates = derive_batch_aggregates(&batches, &jobs);

        assert_eq!(aggregates.len(), 2);
        assert_eq!(aggregates[0].info.id, earlier_id);
        assert_eq!(aggregates[1].info.id, later_id);
    }

    #[test]
    fn compact_drops_only_fully_cleared_batches() {
        let surviving_id = Uuid::from_u128(1);
        let cleared_id = Uuid::from_u128(2);
        let mut batches = BTreeMap::new();
        batches.insert(surviving_id, batch_info(surviving_id, 1, 100, 10));
        batches.insert(cleared_id, batch_info(cleared_id, 1, 100, 10));

        let jobs = vec![job(
            Some(surviving_id),
            TransferJobState::Completed {
                result: CompletionResult::Transferred,
            },
            100,
            100,
            0,
        )];

        compact_batches(&mut batches, &jobs);

        assert!(batches.contains_key(&surviving_id));
        assert!(!batches.contains_key(&cleared_id));
    }

    #[test]
    fn document_without_batches_field_deserializes_empty() {
        let mut value = serde_json::to_value(TransferQueueDocument::default()).unwrap();
        value.as_object_mut().unwrap().remove("batches");

        let document: TransferQueueDocument = serde_json::from_value(value).unwrap();

        assert!(document.batches.is_empty());
    }
}
