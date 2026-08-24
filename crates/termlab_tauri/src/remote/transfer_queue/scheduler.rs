use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use super::model::{
    QueueSettings, TransferJob, TransferJobState, TransferPriority, TransferQueueDocument,
};

const MAX_AUTOMATIC_ATTEMPTS: u8 = 3;

/// An in-flight transfer identity whose resources remain reserved while the
/// scheduler considers new work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLease {
    pub job_id: Uuid,
    pub host_key: String,
    pub destination_key: String,
}

/// Whether a failure is safe to retry automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Transient,
    Permanent,
}

/// Select eligible jobs without mutating the durable queue or starting work.
pub fn select_runnable_jobs(
    jobs: &[TransferJob],
    active: &[ActiveLease],
    settings: &QueueSettings,
    now_ms: u64,
) -> Vec<Uuid> {
    let mut reservations = Reservations::default();
    let mut leased_job_ids = HashSet::new();

    for lease in active {
        if leased_job_ids.insert(lease.job_id) {
            reservations.reserve(&lease.host_key, &lease.destination_key);
        }
    }

    for job in jobs {
        if job.state.holds_lease() && leased_job_ids.insert(job.id) {
            reservations.reserve(&job.host_key, &job.destination_key);
        }
    }

    let mut candidates: Vec<&TransferJob> = jobs
        .iter()
        .filter(|job| !leased_job_ids.contains(&job.id) && is_eligible(job, now_ms))
        .collect();
    candidates.sort_by_key(|job| {
        (
            priority_rank(job.priority),
            job.queue_order,
            job.created_at_ms,
        )
    });

    let mut selected = Vec::new();
    for job in candidates {
        if reservations.can_reserve(job, settings) {
            reservations.reserve(&job.host_key, &job.destination_key);
            selected.push(job.id);
        }
    }
    selected
}

/// Apply durable queue suspension before selecting otherwise pure candidates.
pub fn select_runnable_jobs_from_document(
    document: &TransferQueueDocument,
    active: &[ActiveLease],
    now_ms: u64,
) -> Vec<Uuid> {
    if document.queue_paused {
        Vec::new()
    } else {
        select_runnable_jobs(&document.jobs, active, &document.settings, now_ms)
    }
}

/// Classify text received from transfer infrastructure conservatively: only
/// established transport interruptions are retried automatically.
pub fn classify_failure(error: &str) -> FailureClass {
    let error = error.to_ascii_lowercase();
    if [
        "disconnect",
        "timed out",
        "timeout",
        "connection reset",
        "connection aborted",
        "channel closed",
        "broken pipe",
        "not connected",
        "unexpected eof",
    ]
    .iter()
    .any(|indicator| error.contains(indicator))
    {
        FailureClass::Transient
    } else {
        FailureClass::Permanent
    }
}

/// Return the delay before an automatic retry attempt, if it is allowed.
/// Attempt one is the initial run, so only attempts two and three retry.
pub fn retry_delay_ms(attempt: u8) -> Option<u64> {
    match attempt {
        2 => Some(1_000),
        3 => Some(2_000),
        _ => None,
    }
}

fn is_eligible(job: &TransferJob, now_ms: u64) -> bool {
    match job.state {
        TransferJobState::Queued => true,
        TransferJobState::RetryWaiting {
            attempt,
            next_retry_at_ms,
        } => {
            attempt >= 2
                && attempt <= job.max_attempts.min(MAX_AUTOMATIC_ATTEMPTS)
                && next_retry_at_ms <= now_ms
        }
        _ => false,
    }
}

fn priority_rank(priority: TransferPriority) -> u8 {
    match priority {
        TransferPriority::Interactive => 0,
        TransferPriority::Normal => 1,
    }
}

#[derive(Default)]
struct Reservations {
    total: usize,
    hosts: HashMap<String, usize>,
    destinations: HashSet<String>,
}

impl Reservations {
    fn can_reserve(&self, job: &TransferJob, settings: &QueueSettings) -> bool {
        self.total < settings.global_limit
            && self.hosts.get(&job.host_key).copied().unwrap_or_default() < settings.per_host_limit
            && !self.destinations.contains(&job.destination_key)
    }

    fn reserve(&mut self, host_key: &str, destination_key: &str) {
        self.total += 1;
        *self.hosts.entry(host_key.into()).or_default() += 1;
        self.destinations.insert(destination_key.into());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveLease, FailureClass, classify_failure, retry_delay_ms, select_runnable_jobs,
        select_runnable_jobs_from_document,
    };
    use crate::remote::transfer_queue::model::{
        CommitPhase, ConflictPolicy, QueueSettings, TransferDirection, TransferEndpoint,
        TransferJob, TransferJobState, TransferOrigin, TransferPriority, TransferProtocol,
        TransferQueueDocument, build_destination_key,
    };
    use uuid::Uuid;

    fn job(
        name: &str,
        priority: TransferPriority,
        host_key: &str,
        destination: &str,
        queue_order: u64,
        created_at_ms: u64,
        state: TransferJobState,
    ) -> TransferJob {
        TransferJob {
            id: Uuid::new_v4(),
            protocol: TransferProtocol::Sftp,
            direction: TransferDirection::Upload,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: host_key.into(),
                label: host_key.into(),
            },
            local_path: format!("/local/{name}"),
            remote_path: destination.into(),
            file_name: name.into(),
            batch_id: None,
            priority,
            queue_order,
            host_key: host_key.into(),
            destination_key: build_destination_key(
                host_key,
                &TransferDirection::Upload,
                &format!("/local/{name}"),
                destination,
            ),
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
            created_at_ms,
            updated_at_ms: created_at_ms,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }

    fn document(jobs: Vec<TransferJob>) -> TransferQueueDocument {
        TransferQueueDocument {
            queue_paused: false,
            jobs,
            ..TransferQueueDocument::default()
        }
    }

    fn selected_ids(jobs: &[TransferJob], selected: Vec<Uuid>) -> Vec<String> {
        selected
            .into_iter()
            .map(|id| {
                jobs.iter()
                    .find(|job| job.id == id)
                    .expect("selected job belongs to the document")
                    .file_name
                    .clone()
            })
            .collect()
    }

    #[test]
    fn selection_honors_priority_limits_and_destination_serialization() {
        let jobs = vec![
            job(
                "normal-a",
                TransferPriority::Normal,
                "host-a",
                "/same",
                1,
                10,
                TransferJobState::Queued,
            ),
            job(
                "editor",
                TransferPriority::Interactive,
                "host-a",
                "/editor",
                2,
                20,
                TransferJobState::Queued,
            ),
            job(
                "same-destination",
                TransferPriority::Normal,
                "host-a",
                "/same/./",
                3,
                30,
                TransferJobState::Queued,
            ),
            job(
                "host-cap",
                TransferPriority::Normal,
                "host-a",
                "/third",
                4,
                40,
                TransferJobState::Queued,
            ),
            job(
                "other-host",
                TransferPriority::Normal,
                "host-b",
                "/other",
                5,
                50,
                TransferJobState::Queued,
            ),
        ];
        let chosen = select_runnable_jobs(&jobs, &[], &QueueSettings::default(), 100);

        assert_eq!(
            selected_ids(&jobs, chosen),
            ["editor", "normal-a", "other-host"]
        );
    }

    #[test]
    fn selection_uses_explicit_order_then_creation_time_without_reordering_jobs() {
        let jobs = vec![
            job(
                "later-created",
                TransferPriority::Normal,
                "host-a",
                "/one",
                4,
                90,
                TransferJobState::Queued,
            ),
            job(
                "earlier-created",
                TransferPriority::Normal,
                "host-b",
                "/two",
                4,
                10,
                TransferJobState::Queued,
            ),
            job(
                "first-order",
                TransferPriority::Normal,
                "host-c",
                "/three",
                1,
                80,
                TransferJobState::Queued,
            ),
        ];
        let original_jobs = jobs.clone();
        let chosen = select_runnable_jobs(&jobs, &[], &QueueSettings::default(), 100);

        assert_eq!(
            selected_ids(&jobs, chosen),
            ["first-order", "earlier-created", "later-created"]
        );
        assert_eq!(jobs, original_jobs);
    }

    #[test]
    fn selection_skips_blocked_states_and_retries_before_their_deadline() {
        let jobs = vec![
            job(
                "paused",
                TransferPriority::Interactive,
                "host-a",
                "/one",
                1,
                1,
                TransferJobState::Paused,
            ),
            job(
                "connection",
                TransferPriority::Interactive,
                "host-b",
                "/two",
                2,
                2,
                TransferJobState::NeedsConnection {
                    message: "Reconnect".into(),
                },
            ),
            job(
                "attention",
                TransferPriority::Interactive,
                "host-c",
                "/three",
                3,
                3,
                TransferJobState::NeedsAttention {
                    reason: crate::remote::transfer_queue::model::AttentionReason::MissingPartial,
                },
            ),
            job(
                "retry-later",
                TransferPriority::Interactive,
                "host-d",
                "/four",
                4,
                4,
                TransferJobState::RetryWaiting {
                    attempt: 2,
                    next_retry_at_ms: 101,
                },
            ),
            job(
                "ready",
                TransferPriority::Normal,
                "host-e",
                "/five",
                5,
                5,
                TransferJobState::Queued,
            ),
        ];
        let chosen = select_runnable_jobs(&jobs, &[], &QueueSettings::default(), 100);

        assert_eq!(selected_ids(&jobs, chosen), ["ready"]);
    }

    #[test]
    fn selection_allows_due_retries_but_never_a_fourth_automatic_attempt() {
        let mut exhausted = job(
            "exhausted",
            TransferPriority::Interactive,
            "host-a",
            "/one",
            1,
            1,
            TransferJobState::RetryWaiting {
                attempt: 4,
                next_retry_at_ms: 100,
            },
        );
        exhausted.max_attempts = 3;
        let jobs = vec![
            job(
                "third-attempt",
                TransferPriority::Interactive,
                "host-b",
                "/two",
                2,
                2,
                TransferJobState::RetryWaiting {
                    attempt: 3,
                    next_retry_at_ms: 100,
                },
            ),
            exhausted,
        ];
        let chosen = select_runnable_jobs(&jobs, &[], &QueueSettings::default(), 100);

        assert_eq!(selected_ids(&jobs, chosen), ["third-attempt"]);
    }

    #[test]
    fn active_leases_from_each_running_phase_reserve_capacity() {
        let jobs = vec![
            job(
                "connecting",
                TransferPriority::Normal,
                "host-a",
                "/one",
                1,
                1,
                TransferJobState::Connecting,
            ),
            job(
                "checking",
                TransferPriority::Normal,
                "host-b",
                "/two",
                2,
                2,
                TransferJobState::Checking,
            ),
            job(
                "running",
                TransferPriority::Normal,
                "host-c",
                "/three",
                3,
                3,
                TransferJobState::Running,
            ),
            job(
                "queued",
                TransferPriority::Interactive,
                "host-d",
                "/four",
                4,
                4,
                TransferJobState::Queued,
            ),
        ];
        let chosen = select_runnable_jobs(&jobs, &[], &QueueSettings::default(), 100);

        assert!(chosen.is_empty());
    }

    #[test]
    fn explicit_active_leases_hold_host_and_destination_slots() {
        let jobs = vec![
            job(
                "same-destination",
                TransferPriority::Interactive,
                "host-a",
                "/same",
                1,
                1,
                TransferJobState::Queued,
            ),
            job(
                "host-cap",
                TransferPriority::Normal,
                "host-a",
                "/other",
                2,
                2,
                TransferJobState::Queued,
            ),
            job(
                "other-host",
                TransferPriority::Normal,
                "host-b",
                "/third",
                3,
                3,
                TransferJobState::Queued,
            ),
        ];
        let active = vec![
            ActiveLease {
                job_id: Uuid::new_v4(),
                host_key: "host-a".into(),
                destination_key: build_destination_key(
                    "host-a",
                    &TransferDirection::Upload,
                    "/local",
                    "/same",
                ),
            },
            ActiveLease {
                job_id: Uuid::new_v4(),
                host_key: "host-a".into(),
                destination_key: build_destination_key(
                    "host-a",
                    &TransferDirection::Upload,
                    "/local",
                    "/active",
                ),
            },
        ];

        let chosen = select_runnable_jobs(&jobs, &active, &QueueSettings::default(), 100);

        assert_eq!(selected_ids(&jobs, chosen), ["other-host"]);
    }

    #[test]
    fn a_queued_job_with_an_active_lease_id_is_not_redispatched() {
        let jobs = vec![job(
            "already-running",
            TransferPriority::Interactive,
            "host-a",
            "/same",
            1,
            1,
            TransferJobState::Queued,
        )];
        let active = vec![ActiveLease {
            job_id: jobs[0].id,
            host_key: "host-b".into(),
            destination_key: build_destination_key(
                "host-b",
                &TransferDirection::Upload,
                "/local/other",
                "/other",
            ),
        }];

        let chosen = select_runnable_jobs(&jobs, &active, &QueueSettings::default(), 100);

        assert!(chosen.is_empty());
    }

    #[test]
    fn a_paused_queue_selects_no_jobs() {
        let jobs = vec![job(
            "ready",
            TransferPriority::Interactive,
            "host-a",
            "/one",
            1,
            1,
            TransferJobState::Queued,
        )];
        let mut document = document(jobs);
        document.queue_paused = true;

        assert!(select_runnable_jobs_from_document(&document, &[], 100).is_empty());
    }

    #[test]
    fn failure_classification_separates_retryable_transport_errors() {
        for error in [
            "Disconnected from remote",
            "connection timed out",
            "Connection reset by peer",
        ] {
            assert_eq!(classify_failure(error), FailureClass::Transient);
        }
        for error in [
            "authentication failed",
            "permission denied",
            "source mismatch",
            "destination conflict",
            "missing partial",
            "commit ambiguity",
        ] {
            assert_eq!(classify_failure(error), FailureClass::Permanent);
        }
    }

    #[test]
    fn retry_delay_only_permits_second_and_third_attempts() {
        assert_eq!(retry_delay_ms(1), None);
        assert_eq!(retry_delay_ms(2), Some(1_000));
        assert_eq!(retry_delay_ms(3), Some(2_000));
        assert_eq!(retry_delay_ms(4), None);
    }
}
