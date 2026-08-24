use std::fmt;

use termlab_remote::transfer::SourceFingerprint;

use super::model::{
    AttentionReason, CommitPhase, CompletionResult, ConflictResolution, ManagedArtifacts,
    TransferDirection, TransferJob, TransferJobState,
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
        speed_bytes_per_second: u64,
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
    Restart,
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
            next.speed_bytes_per_second = speed_bytes_per_second;
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
            TransferJobState::Checking | TransferJobState::Running,
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
        (TransferJobState::NeedsAttention { .. }, JobEvent::Resolve(resolution)) => {
            apply_resolution(&mut next, resolution, now_ms);
        }
        (TransferJobState::NeedsAttention { .. }, JobEvent::Restart) => {
            reset_attempt(&mut next);
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

fn apply_resolution(job: &mut TransferJob, resolution: ConflictResolution, now_ms: u64) {
    match resolution {
        ConflictResolution::Resume => job.state = TransferJobState::Queued,
        ConflictResolution::Overwrite => reset_attempt(job),
        ConflictResolution::Rename { destination } => {
            match job.direction {
                TransferDirection::Upload => job.remote_path = destination.clone(),
                TransferDirection::Download => job.local_path = destination.clone(),
            }
            job.destination_key = destination;
            reset_attempt(job);
        }
        ConflictResolution::Skip => {
            job.state = TransferJobState::Completed {
                result: CompletionResult::Skipped,
            };
            job.finished_at_ms = Some(now_ms);
            clear_live_progress(job);
        }
        ConflictResolution::Restart => reset_attempt(job),
    }
}

fn reset_attempt(job: &mut TransferJob) {
    job.state = TransferJobState::Queued;
    job.source_fingerprint = None;
    job.durable_checkpoint = 0;
    job.bytes_transferred = 0;
    job.total_bytes = 0;
    job.retry_attempt = 0;
    job.artifacts = None;
    job.commit_phase = CommitPhase::None;
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
        JobEvent::Restart => "restart",
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
        let restarted = reduce_job(&attention, JobEvent::Restart, 61).unwrap();

        assert_eq!(restarted.state, TransferJobState::Queued);
        assert_eq!(restarted.durable_checkpoint, 0);
        assert_eq!(restarted.bytes_transferred, 0);
        assert_eq!(restarted.source_fingerprint, None);
        assert_eq!(restarted.artifacts, None);
        assert_eq!(restarted.commit_phase, CommitPhase::None);
    }

    #[test]
    fn progress_does_not_advance_the_durable_checkpoint() {
        let mut running = sample_job(TransferJobState::Running);
        running.durable_checkpoint = 1_024;

        let progressed = reduce_job(
            &running,
            JobEvent::Progress {
                bytes: 2_048,
                speed_bytes_per_second: 512,
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
                        speed_bytes_per_second: 1,
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
