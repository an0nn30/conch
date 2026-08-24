use std::fmt;

use termlab_remote::transfer::SourceFingerprint;

use super::model::{
    AttentionReason, CommitPhase, CompletionResult, ConflictPolicy, ConflictResolution,
    ManagedArtifacts, TransferDirection, TransferJob, TransferJobState, build_destination_key,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    BeginConnect,
    BeginCheck,
    BeginRun {
        fingerprint: SourceFingerprint,
        total_bytes: u64,
        artifacts: ManagedArtifacts,
    },
    Progress {
        bytes: u64,
        speed_bytes_per_second: Option<u64>,
        eta_seconds: Option<u64>,
    },
    Checkpoint {
        bytes: u64,
    },
    Pause,
    Resume,
    NeedsConnection(String),
    NeedsAttention(AttentionReason),
    RetryScheduled {
        attempt: u8,
        next_retry_at_ms: u64,
    },
    RetryReady,
    ManualRetry,
    Resolve(ConflictResolution),
    ResolveAfterCleanup(ConflictResolution),
    Complete(CompletionResult),
    Fail(String),
    Cancel(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    pub state: &'static str,
    pub event: &'static str,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "event {} is not legal from transfer state {}",
            self.event, self.state
        )
    }
}

impl std::error::Error for TransitionError {}

pub fn reduce_job(
    job: &TransferJob,
    event: JobEvent,
    now_ms: u64,
) -> Result<TransferJob, TransitionError> {
    let state_name = state_name(&job.state);
    let event_name = event_name(&event);
    let mut next = job.clone();

    match (&job.state, event) {
        (TransferJobState::Queued, JobEvent::BeginConnect) => {
            next.state = TransferJobState::Connecting;
            next.started_at_ms.get_or_insert(now_ms);
            next.finished_at_ms = None;
        }
        (TransferJobState::Connecting, JobEvent::BeginCheck) => {
            next.state = TransferJobState::Checking;
        }
        (
            TransferJobState::Checking,
            JobEvent::BeginRun {
                fingerprint,
                total_bytes,
                artifacts,
            },
        ) => {
            next.state = TransferJobState::Running;
            next.source_fingerprint = Some(fingerprint);
            next.total_bytes = total_bytes;
            next.bytes_transferred = next.durable_checkpoint;
            next.speed_bytes_per_second = 0;
            next.eta_seconds = None;
            next.artifacts = Some(artifacts);
        }
        (
            TransferJobState::Running,
            JobEvent::Progress {
                bytes,
                speed_bytes_per_second,
                eta_seconds,
            },
        ) => {
            next.bytes_transferred = bytes;
            next.speed_bytes_per_second = speed_bytes_per_second.unwrap_or(0);
            next.eta_seconds = eta_seconds;
        }
        (TransferJobState::Running, JobEvent::Checkpoint { bytes }) => {
            next.durable_checkpoint = bytes;
        }
        (
            TransferJobState::Queued
            | TransferJobState::Connecting
            | TransferJobState::Checking
            | TransferJobState::Running
            | TransferJobState::RetryWaiting { .. },
            JobEvent::Pause,
        ) => {
            next.state = TransferJobState::Paused;
            clear_live_progress(&mut next);
        }
        (TransferJobState::Paused, JobEvent::Resume) => {
            next.state = TransferJobState::Queued;
        }
        (
            TransferJobState::Connecting | TransferJobState::Checking | TransferJobState::Running,
            JobEvent::NeedsConnection(message),
        ) => {
            next.state = TransferJobState::NeedsConnection { message };
            clear_live_progress(&mut next);
        }
        (
            TransferJobState::Queued
            | TransferJobState::Checking
            | TransferJobState::Running
            | TransferJobState::Paused
            | TransferJobState::NeedsConnection { .. }
            | TransferJobState::NeedsAttention { .. },
            JobEvent::NeedsAttention(reason),
        ) => {
            next.state = TransferJobState::NeedsAttention { reason };
            clear_live_progress(&mut next);
        }
        (
            TransferJobState::Connecting | TransferJobState::Checking | TransferJobState::Running,
            JobEvent::RetryScheduled {
                attempt,
                next_retry_at_ms,
            },
        ) if attempt > 0 && attempt <= job.max_attempts => {
            next.state = TransferJobState::RetryWaiting {
                attempt,
                next_retry_at_ms,
            };
            next.retry_attempt = attempt;
            clear_live_progress(&mut next);
        }
        (TransferJobState::RetryWaiting { .. }, JobEvent::RetryReady) => {
            next.state = TransferJobState::Queued;
        }
        (
            TransferJobState::Failed { .. } | TransferJobState::NeedsConnection { .. },
            JobEvent::ManualRetry,
        ) => {
            next.state = TransferJobState::Queued;
            next.retry_attempt = 0;
            next.finished_at_ms = None;
            clear_live_progress(&mut next);
        }
        (TransferJobState::NeedsAttention { reason }, JobEvent::Resolve(resolution))
            if resolution_is_legal(reason, &resolution)
                && (!resolution_abandons_attempt(&resolution) || !job_owns_attempt(job)) =>
        {
            apply_resolution(&mut next, resolution, now_ms);
        }
        (
            TransferJobState::NeedsAttention { reason },
            JobEvent::ResolveAfterCleanup(resolution),
        ) if resolution_is_legal(reason, &resolution)
            && resolution_abandons_attempt(&resolution) =>
        {
            apply_resolution(&mut next, resolution, now_ms);
        }
        (TransferJobState::Running, JobEvent::Complete(result)) => {
            next.state = TransferJobState::Completed { result };
            next.commit_phase = CommitPhase::Complete;
            next.finished_at_ms = Some(now_ms);
            clear_live_progress(&mut next);
        }
        (state, JobEvent::Fail(error)) if !state.is_terminal() => {
            next.state = TransferJobState::Failed { error };
            next.finished_at_ms = Some(now_ms);
            clear_live_progress(&mut next);
        }
        (state, JobEvent::Cancel(cleanup_error)) if !state.is_terminal() => {
            next.state = TransferJobState::Cancelled { cleanup_error };
            next.finished_at_ms = Some(now_ms);
            clear_live_progress(&mut next);
        }
        _ => {
            return Err(TransitionError {
                state: state_name,
                event: event_name,
            });
        }
    }

    next.updated_at_ms = now_ms;
    Ok(next)
}

pub(crate) fn resolution_is_legal(
    reason: &AttentionReason,
    resolution: &ConflictResolution,
) -> bool {
    match (reason, resolution) {
        (
            AttentionReason::DestinationConflict {
                resume_available: true,
            },
            ConflictResolution::Resume,
        )
        | (
            AttentionReason::DestinationConflict { .. },
            ConflictResolution::Overwrite | ConflictResolution::Rename { .. },
        )
        | (_, ConflictResolution::Restart | ConflictResolution::Skip) => true,
        _ => false,
    }
}

pub(crate) fn resolution_abandons_attempt(resolution: &ConflictResolution) -> bool {
    !matches!(resolution, ConflictResolution::Resume)
}

pub(crate) fn job_owns_attempt(job: &TransferJob) -> bool {
    job.artifacts.is_some() || job.durable_checkpoint != 0 || job.commit_phase != CommitPhase::None
}

fn apply_resolution(job: &mut TransferJob, resolution: ConflictResolution, now_ms: u64) {
    match resolution {
        ConflictResolution::Resume => {
            job.state = TransferJobState::Queued;
            job.conflict_policy = ConflictPolicy::Overwrite;
        }
        ConflictResolution::Overwrite => {
            reset_attempt(job);
            job.conflict_policy = ConflictPolicy::Overwrite;
        }
        ConflictResolution::Rename { destination } => {
            match job.direction {
                TransferDirection::Upload => job.remote_path = destination.clone(),
                TransferDirection::Download => job.local_path = destination.clone(),
            }
            job.destination_key = build_destination_key(
                &job.host_key,
                &job.direction,
                &job.local_path,
                &job.remote_path,
            );
            reset_attempt(job);
            job.conflict_policy = ConflictPolicy::Ask;
        }
        ConflictResolution::Skip => {
            clear_attempt_identity(job);
            job.state = TransferJobState::Completed {
                result: CompletionResult::Skipped,
            };
            job.finished_at_ms = Some(now_ms);
        }
        ConflictResolution::Restart => reset_attempt(job),
    }
}

fn reset_attempt(job: &mut TransferJob) {
    job.state = TransferJobState::Queued;
    clear_attempt_identity(job);
}

fn clear_attempt_identity(job: &mut TransferJob) {
    job.source_fingerprint = None;
    job.durable_checkpoint = 0;
    job.bytes_transferred = 0;
    job.total_bytes = 0;
    job.retry_attempt = 0;
    job.artifacts = None;
    job.commit_phase = CommitPhase::None;
    job.commit_backup_expected = None;
    job.started_at_ms = None;
    job.finished_at_ms = None;
    clear_live_progress(job);
}

fn clear_live_progress(job: &mut TransferJob) {
    job.speed_bytes_per_second = 0;
    job.eta_seconds = None;
}

fn state_name(state: &TransferJobState) -> &'static str {
    match state {
        TransferJobState::Queued => "queued",
        TransferJobState::Connecting => "connecting",
        TransferJobState::Checking => "checking",
        TransferJobState::Running => "running",
        TransferJobState::Paused => "paused",
        TransferJobState::NeedsConnection { .. } => "needsConnection",
        TransferJobState::NeedsAttention { .. } => "needsAttention",
        TransferJobState::RetryWaiting { .. } => "retryWaiting",
        TransferJobState::Completed { .. } => "completed",
        TransferJobState::Failed { .. } => "failed",
        TransferJobState::Cancelled { .. } => "cancelled",
    }
}

fn event_name(event: &JobEvent) -> &'static str {
    match event {
        JobEvent::BeginConnect => "beginConnect",
        JobEvent::BeginCheck => "beginCheck",
        JobEvent::BeginRun { .. } => "beginRun",
        JobEvent::Progress { .. } => "progress",
        JobEvent::Checkpoint { .. } => "checkpoint",
        JobEvent::Pause => "pause",
        JobEvent::Resume => "resume",
        JobEvent::NeedsConnection(_) => "needsConnection",
        JobEvent::NeedsAttention(_) => "needsAttention",
        JobEvent::RetryScheduled { .. } => "retryScheduled",
        JobEvent::RetryReady => "retryReady",
        JobEvent::ManualRetry => "manualRetry",
        JobEvent::Resolve(_) => "resolve",
        JobEvent::ResolveAfterCleanup(_) => "resolveAfterCleanup",
        JobEvent::Complete(_) => "complete",
        JobEvent::Fail(_) => "fail",
        JobEvent::Cancel(_) => "cancel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::transfer_queue::model::*;
    use termlab_remote::transfer::SourceFingerprint;
    use uuid::Uuid;

    fn fingerprint(token: &str) -> SourceFingerprint {
        SourceFingerprint {
            size: 8_192,
            modified_token: Some(token.into()),
        }
    }

    fn artifacts() -> ManagedArtifacts {
        ManagedArtifacts {
            partial_path: "/tmp/.report.csv.partial".into(),
            backup_path: "/tmp/.report.csv.backup".into(),
        }
    }

    fn sample_job(state: TransferJobState) -> TransferJob {
        TransferJob {
            id: Uuid::nil(),
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
            batch_id: None,
            priority: TransferPriority::Normal,
            queue_order: 4,
            host_key: "configured:server-1".into(),
            destination_key: "configured:server-1:/srv/report.csv".into(),
            state,
            source_fingerprint: None,
            durable_checkpoint: 0,
            bytes_transferred: 0,
            total_bytes: 0,
            speed_bytes_per_second: 0,
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

    fn source_changed() -> AttentionReason {
        AttentionReason::SourceChanged {
            expected: fingerprint("unixNs:12"),
            actual: fingerprint("unixNs:13"),
        }
    }

    #[test]
    fn happy_path_moves_from_queue_to_completed() {
        let queued = sample_job(TransferJobState::Queued);
        let connecting = reduce_job(&queued, JobEvent::BeginConnect, 20).unwrap();
        assert_eq!(connecting.state, TransferJobState::Connecting);
        assert_eq!(connecting.started_at_ms, Some(20));

        let checking = reduce_job(&connecting, JobEvent::BeginCheck, 21).unwrap();
        assert_eq!(checking.state, TransferJobState::Checking);

        let running = reduce_job(
            &checking,
            JobEvent::BeginRun {
                fingerprint: fingerprint("unixNs:12"),
                total_bytes: 8_192,
                artifacts: artifacts(),
            },
            22,
        )
        .unwrap();
        assert_eq!(running.state, TransferJobState::Running);
        assert_eq!(running.source_fingerprint, Some(fingerprint("unixNs:12")));
        assert_eq!(running.total_bytes, 8_192);
        assert_eq!(running.artifacts, Some(artifacts()));

        let completed = reduce_job(
            &running,
            JobEvent::Complete(CompletionResult::Transferred),
            23,
        )
        .unwrap();
        assert_eq!(
            completed.state,
            TransferJobState::Completed {
                result: CompletionResult::Transferred,
            }
        );
        assert_eq!(completed.finished_at_ms, Some(23));
    }

    #[test]
    fn pause_and_resume_move_running_job_back_to_queue() {
        let running = sample_job(TransferJobState::Running);
        let paused = reduce_job(&running, JobEvent::Pause, 30).unwrap();
        assert_eq!(paused.state, TransferJobState::Paused);

        let resumed = reduce_job(&paused, JobEvent::Resume, 31).unwrap();
        assert_eq!(resumed.state, TransferJobState::Queued);
    }

    #[test]
    fn connection_loss_from_checking_requires_explicit_connection() {
        let checking = sample_job(TransferJobState::Checking);
        let disconnected = reduce_job(
            &checking,
            JobEvent::NeedsConnection("Reconnect explicitly".into()),
            40,
        )
        .unwrap();
        assert_eq!(
            disconnected.state,
            TransferJobState::NeedsConnection {
                message: "Reconnect explicitly".into(),
            }
        );
    }

    #[test]
    fn checking_and_running_can_require_attention() {
        for state in [TransferJobState::Checking, TransferJobState::Running] {
            let job = sample_job(state);
            let attention = reduce_job(
                &job,
                JobEvent::NeedsAttention(AttentionReason::MissingPartial),
                41,
            )
            .unwrap();
            assert_eq!(
                attention.state,
                TransferJobState::NeedsAttention {
                    reason: AttentionReason::MissingPartial,
                }
            );
        }
    }

    #[test]
    fn transient_failure_waits_then_returns_to_queue() {
        let running = sample_job(TransferJobState::Running);
        let waiting = reduce_job(
            &running,
            JobEvent::RetryScheduled {
                attempt: 2,
                next_retry_at_ms: 5_000,
            },
            50,
        )
        .unwrap();
        assert_eq!(
            waiting.state,
            TransferJobState::RetryWaiting {
                attempt: 2,
                next_retry_at_ms: 5_000,
            }
        );
        assert_eq!(waiting.retry_attempt, 2);

        let ready = reduce_job(&waiting, JobEvent::RetryReady, 51).unwrap();
        assert_eq!(ready.state, TransferJobState::Queued);
    }

    #[test]
    fn restart_after_source_change_clears_attempt_identity() {
        let mut job = sample_job(TransferJobState::Running);
        job.durable_checkpoint = 4_096;
        job.bytes_transferred = 4_096;
        job.source_fingerprint = Some(fingerprint("unixNs:12"));
        job.artifacts = Some(artifacts());
        job.commit_phase = CommitPhase::Prepared;

        let attention = reduce_job(&job, JobEvent::NeedsAttention(source_changed()), 60).unwrap();
        let restarted = reduce_job(
            &attention,
            JobEvent::ResolveAfterCleanup(ConflictResolution::Restart),
            61,
        )
        .unwrap();

        assert_eq!(restarted.state, TransferJobState::Queued);
        assert_eq!(restarted.durable_checkpoint, 0);
        assert_eq!(restarted.bytes_transferred, 0);
        assert_eq!(restarted.source_fingerprint, None);
        assert_eq!(restarted.artifacts, None);
        assert_eq!(restarted.commit_phase, CommitPhase::None);
    }

    fn assert_resume_is_rejected(reason: AttentionReason) {
        let mut job = sample_job(TransferJobState::NeedsAttention { reason });
        job.durable_checkpoint = 4_096;
        job.source_fingerprint = Some(fingerprint("unixNs:12"));
        job.artifacts = Some(artifacts());

        let result = reduce_job(&job, JobEvent::Resolve(ConflictResolution::Resume), 62);
        assert!(result.is_err());
    }

    #[test]
    fn source_changed_cannot_resume_old_attempt_identity() {
        assert_resume_is_rejected(source_changed());
    }

    #[test]
    fn source_that_cannot_be_verified_cannot_resume() {
        assert_resume_is_rejected(AttentionReason::SourceCannotResume);
    }

    #[test]
    fn missing_partial_cannot_resume() {
        assert_resume_is_rejected(AttentionReason::MissingPartial);
    }

    #[test]
    fn overwrite_resolution_resets_identity_and_persists_overwrite_authorization() {
        let mut job = sample_job(TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: false,
            },
        });
        job.source_fingerprint = Some(fingerprint("unixNs:12"));
        job.durable_checkpoint = 4_096;
        job.artifacts = Some(artifacts());

        assert!(
            reduce_job(&job, JobEvent::Resolve(ConflictResolution::Overwrite), 63).is_err(),
            "owned attempt identity may only be abandoned after cleanup acknowledges"
        );
        let resolved = reduce_job(
            &job,
            JobEvent::ResolveAfterCleanup(ConflictResolution::Overwrite),
            63,
        )
        .unwrap();

        assert_eq!(resolved.state, TransferJobState::Queued);
        assert_eq!(resolved.conflict_policy, ConflictPolicy::Overwrite);
        assert_eq!(resolved.source_fingerprint, None);
        assert_eq!(resolved.durable_checkpoint, 0);
        assert_eq!(resolved.artifacts, None);
    }

    #[test]
    fn resume_resolution_retains_identity_and_persists_overwrite_authorization() {
        let mut job = sample_job(TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: true,
            },
        });
        job.source_fingerprint = Some(fingerprint("unixNs:12"));
        job.durable_checkpoint = 4_096;
        job.artifacts = Some(artifacts());

        let resolved = reduce_job(&job, JobEvent::Resolve(ConflictResolution::Resume), 63).unwrap();

        assert_eq!(resolved.state, TransferJobState::Queued);
        assert_eq!(resolved.conflict_policy, ConflictPolicy::Overwrite);
        assert_eq!(resolved.source_fingerprint, job.source_fingerprint);
        assert_eq!(resolved.durable_checkpoint, 4_096);
        assert_eq!(resolved.artifacts, job.artifacts);
    }

    #[test]
    fn rename_uses_the_same_scoped_key_as_an_equivalent_new_job() {
        let mut conflict = sample_job(TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: false,
            },
        });
        conflict.conflict_policy = ConflictPolicy::Overwrite;
        let renamed = reduce_job(
            &conflict,
            JobEvent::Resolve(ConflictResolution::Rename {
                destination: "/srv/releases/.././report.csv".into(),
            }),
            63,
        )
        .unwrap();
        let mut equivalent_new_job = sample_job(TransferJobState::Queued);
        equivalent_new_job.remote_path = "/srv/report.csv".into();
        equivalent_new_job.destination_key = build_destination_key(
            &equivalent_new_job.host_key,
            &equivalent_new_job.direction,
            &equivalent_new_job.local_path,
            &equivalent_new_job.remote_path,
        );

        assert_eq!(
            renamed.destination_key, equivalent_new_job.destination_key,
            "equivalent destinations must collide in scheduler serialization"
        );
        assert_eq!(
            renamed.destination_key,
            "configured:server-1:/srv/report.csv"
        );
        assert_eq!(renamed.conflict_policy, ConflictPolicy::Ask);

        let mut other_endpoint_job = equivalent_new_job;
        other_endpoint_job.host_key = "configured:server-2".into();
        other_endpoint_job.destination_key = build_destination_key(
            &other_endpoint_job.host_key,
            &other_endpoint_job.direction,
            &other_endpoint_job.local_path,
            &other_endpoint_job.remote_path,
        );
        assert_ne!(renamed.destination_key, other_endpoint_job.destination_key);
    }

    #[test]
    fn progress_does_not_advance_the_durable_checkpoint() {
        let mut running = sample_job(TransferJobState::Running);
        running.durable_checkpoint = 1_024;

        let progressed = reduce_job(
            &running,
            JobEvent::Progress {
                bytes: 2_048,
                speed_bytes_per_second: Some(512),
                eta_seconds: Some(12),
            },
            70,
        )
        .unwrap();
        assert_eq!(progressed.bytes_transferred, 2_048);
        assert_eq!(progressed.durable_checkpoint, 1_024);

        let checkpointed =
            reduce_job(&progressed, JobEvent::Checkpoint { bytes: 2_048 }, 71).unwrap();
        assert_eq!(checkpointed.durable_checkpoint, 2_048);
    }

    #[test]
    fn terminal_states_reject_further_progress() {
        let states = [
            TransferJobState::Completed {
                result: CompletionResult::Transferred,
            },
            TransferJobState::Failed {
                error: "disk full".into(),
            },
            TransferJobState::Cancelled {
                cleanup_error: None,
            },
        ];

        for state in states {
            let job = sample_job(state);
            assert!(
                reduce_job(
                    &job,
                    JobEvent::Progress {
                        bytes: 1,
                        speed_bytes_per_second: Some(1),
                        eta_seconds: None,
                    },
                    80,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn queued_job_cannot_complete_without_running() {
        let queued = sample_job(TransferJobState::Queued);
        assert!(
            reduce_job(
                &queued,
                JobEvent::Complete(CompletionResult::Transferred),
                90,
            )
            .is_err()
        );
    }
}
