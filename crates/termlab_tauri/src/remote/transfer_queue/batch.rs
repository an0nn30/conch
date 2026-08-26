use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::model::{TransferDirection, TransferJob, TransferJobState};

/// The expansion-state reason a batch carries once its owner cancels it. The
/// engine writes it when the cancel lands and the expansion task writes the
/// same string when its walk unwinds, so the two never disagree.
pub const BATCH_CANCELLED_REASON: &str = "batch cancelled";

/// The stop flag shared between `cancel_batch` and the expansion task that is
/// walking that batch's tree. The task polls it through `walk_tree`'s
/// `cancelled()`, which is why it must be cheap and lock-free.
#[derive(Debug, Clone, Default)]
pub struct BatchCancellation(Arc<AtomicBool>);

impl BatchCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

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
    /// Monotonic count of members that have entered `Completed`, accrued once
    /// per member at the engine's serialization point. Member jobs age out of
    /// history (the 500-row cap) and can be cleared by hand, so a roll-up
    /// derived from surviving jobs alone would plateau and then count DOWN
    /// while the batch was still progressing. This is the small persisted
    /// record that keeps `files_done` honest once members are gone.
    #[serde(default)]
    #[ts(as = "f64")]
    pub completed_files: u64,
    /// Bytes attributed to those completed members, on the same terms.
    #[serde(default)]
    #[ts(as = "f64")]
    pub completed_bytes: u64,
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

            // Members leave the job list — the history cap trims them, and
            // Clear-completed removes them outright — while the totals come
            // from the persisted `discovered_*`. Reconciling against the
            // monotonic per-batch record is what stops the roll-up from
            // plateauing and then counting DOWN mid-batch. The live sums
            // still win while they lead, because only they can see the
            // partial bytes of a member that is still running.
            files_done = files_done.max(info.completed_files);
            bytes_done = bytes_done.max(info.completed_bytes);

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

/// Accrue the monotonic completion counters for every member that entered
/// `Completed` between `previous_jobs` and `next_jobs`. Called once per
/// commit, from the engine's single serialization point, so each member is
/// counted exactly once: `Completed` is a terminal state the reducer never
/// leaves (only `Failed`/`NeedsConnection` accept `ManualRetry`), so a member
/// can never re-enter it and double-count.
///
/// Bytes are attributed from the member's own total, falling back to what it
/// actually moved when the total is the "unknown" zero sentinel — never an
/// invented figure.
pub fn accrue_completed_members(
    batches: &mut BTreeMap<Uuid, BatchInfo>,
    previous_jobs: &[TransferJob],
    next_jobs: &[TransferJob],
) {
    for job in next_jobs {
        let Some(batch_id) = job.batch_id else {
            continue;
        };
        if !matches!(job.state, TransferJobState::Completed { .. }) {
            continue;
        }
        let was_completed = previous_jobs.iter().any(|previous| {
            previous.id == job.id && matches!(previous.state, TransferJobState::Completed { .. })
        });
        if was_completed {
            continue;
        }
        let Some(batch) = batches.get_mut(&batch_id) else {
            continue;
        };
        let bytes = if job.total_bytes > 0 {
            job.total_bytes
        } else {
            job.bytes_transferred
        };
        batch.completed_files = batch.completed_files.saturating_add(1);
        batch.completed_bytes = batch.completed_bytes.saturating_add(bytes);
    }
}

/// Drop batches with no remaining member jobs (active or history) so the
/// batch map does not grow without bound as jobs age out of history. A batch
/// whose expansion is still `Running` is never memberless-by-attrition — it
/// simply has not enqueued its first file yet — so it is kept.
pub fn compact_batches(batches: &mut BTreeMap<Uuid, BatchInfo>, jobs: &[TransferJob]) {
    batches.retain(|id, info| {
        info.expansion == BatchExpansion::Running
            || jobs.iter().any(|job| job.batch_id == Some(*id))
    });
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::super::model::{
        CommitPhase, CompletionResult, ConflictPolicy, TransferDirection, TransferEndpoint,
        TransferJob, TransferJobState, TransferOrigin, TransferPriority, TransferProtocol,
        TransferQueueDocument,
    };
    use super::{
        BatchExpansion, BatchInfo, accrue_completed_members, compact_batches,
        derive_batch_aggregates,
    };
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
            completed_files: 0,
            completed_bytes: 0,
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
    fn derive_prefers_persisted_completion_when_members_left_history() {
        let batch_id = Uuid::from_u128(1);
        let mut info = batch_info(batch_id, 2_000, 200_000, 10);
        info.completed_files = 900;
        info.completed_bytes = 90_000;
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, info);

        // Only one member survived the history cap; the other 899 completed
        // members were compacted away.
        let jobs = vec![job(
            Some(batch_id),
            TransferJobState::Completed {
                result: CompletionResult::Transferred,
            },
            100,
            100,
            0,
        )];

        let aggregates = derive_batch_aggregates(&batches, &jobs);

        assert_eq!(
            aggregates[0].files_done, 900,
            "a roll-up must never count DOWN when completed members age out of history"
        );
        assert_eq!(
            aggregates[0].bytes_done, 90_000,
            "persisted completed bytes outrank what the surviving members can still show"
        );
    }

    #[test]
    fn derive_prefers_live_jobs_when_they_lead_the_persisted_record() {
        let batch_id = Uuid::from_u128(1);
        let mut info = batch_info(batch_id, 2, 200, 10);
        info.completed_files = 1;
        info.completed_bytes = 100;
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, info);

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
            // In-flight bytes are only visible on the live job, never in the
            // persisted per-member record.
            job(Some(batch_id), TransferJobState::Running, 40, 100, 50),
        ];

        let aggregates = derive_batch_aggregates(&batches, &jobs);

        assert_eq!(aggregates[0].files_done, 1);
        assert_eq!(
            aggregates[0].bytes_done, 140,
            "partial progress of a running member must still be reflected"
        );
        assert_eq!(
            aggregates[0].eta_seconds,
            Some(1),
            "eta is computed from the reconciled bytes_done, not the raw job sum"
        );
    }

    #[test]
    fn accrue_counts_each_member_completion_exactly_once() {
        let batch_id = Uuid::from_u128(1);
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, batch_info(batch_id, 2, 300, 10));

        let running = job(Some(batch_id), TransferJobState::Running, 40, 100, 50);
        let mut finished = running.clone();
        finished.state = TransferJobState::Completed {
            result: CompletionResult::Transferred,
        };
        finished.bytes_transferred = 100;

        accrue_completed_members(
            &mut batches,
            std::slice::from_ref(&running),
            std::slice::from_ref(&finished),
        );
        assert_eq!(batches[&batch_id].completed_files, 1);
        assert_eq!(batches[&batch_id].completed_bytes, 100);

        // A later commit that merely re-serializes the same terminal job must
        // not accrue it a second time.
        accrue_completed_members(
            &mut batches,
            std::slice::from_ref(&finished),
            std::slice::from_ref(&finished),
        );
        assert_eq!(batches[&batch_id].completed_files, 1);
        assert_eq!(batches[&batch_id].completed_bytes, 100);
    }

    #[test]
    fn accrue_ignores_batchless_and_unknown_batch_members() {
        let batch_id = Uuid::from_u128(1);
        let unknown_batch_id = Uuid::from_u128(2);
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, batch_info(batch_id, 1, 100, 10));

        let completed = TransferJobState::Completed {
            result: CompletionResult::Transferred,
        };
        let batchless = job(None, completed.clone(), 100, 100, 0);
        let stranger = job(Some(unknown_batch_id), completed, 100, 100, 0);

        accrue_completed_members(&mut batches, &[], &[batchless, stranger]);

        assert_eq!(batches[&batch_id].completed_files, 0);
        assert_eq!(batches[&batch_id].completed_bytes, 0);
    }

    #[test]
    fn accrue_uses_the_member_total_when_a_skip_cleared_its_byte_counters() {
        let batch_id = Uuid::from_u128(1);
        let mut batches = BTreeMap::new();
        batches.insert(batch_id, batch_info(batch_id, 1, 100, 10));

        // A skipped member reports zero bytes on both counters, so it accrues
        // a file but no bytes rather than an invented total.
        let queued = job(Some(batch_id), TransferJobState::Queued, 0, 0, 0);
        let mut skipped = queued.clone();
        skipped.state = TransferJobState::Completed {
            result: CompletionResult::Skipped,
        };

        accrue_completed_members(&mut batches, &[queued], &[skipped]);

        assert_eq!(batches[&batch_id].completed_files, 1);
        assert_eq!(batches[&batch_id].completed_bytes, 0);
    }

    #[test]
    fn batch_info_without_completion_counters_deserializes_to_zero() {
        let batch_id = Uuid::from_u128(1);
        let mut value = serde_json::to_value(batch_info(batch_id, 3, 300, 10)).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("completedFiles");
        object.remove("completedBytes");

        let info: BatchInfo = serde_json::from_value(value).unwrap();

        assert_eq!(
            info.completed_files, 0,
            "schema v1 records predate the field"
        );
        assert_eq!(info.completed_bytes, 0);
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
    fn compact_keeps_a_batch_whose_expansion_is_still_running() {
        let expanding_id = Uuid::from_u128(1);
        let finished_id = Uuid::from_u128(2);
        let mut batches = BTreeMap::new();
        let mut expanding = batch_info(expanding_id, 0, 0, 10);
        expanding.expansion = BatchExpansion::Running;
        batches.insert(expanding_id, expanding);
        batches.insert(finished_id, batch_info(finished_id, 1, 100, 10));

        compact_batches(&mut batches, &[]);

        assert!(
            batches.contains_key(&expanding_id),
            "a batch whose first member has not been enqueued yet is not an orphan"
        );
        assert!(!batches.contains_key(&finished_id));
    }

    #[test]
    fn cancellation_flag_is_shared_by_clones() {
        use super::BatchCancellation;

        let cancellation = BatchCancellation::new();
        let observer = cancellation.clone();

        assert!(!observer.is_cancelled());
        cancellation.cancel();

        assert!(
            observer.is_cancelled(),
            "the expansion task polls the same flag the engine flips"
        );
    }

    #[test]
    fn document_without_batches_field_deserializes_empty() {
        let mut value = serde_json::to_value(TransferQueueDocument::default()).unwrap();
        value.as_object_mut().unwrap().remove("batches");

        let document: TransferQueueDocument = serde_json::from_value(value).unwrap();

        assert!(document.batches.is_empty());
    }
}
