use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use parking_lot::RwLock;
use termlab_remote::transfer::SourceFingerprint;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use super::{
    events::{
        QueueEventPayload, QueueSummaryPayload, RunnerEvent, TransferEventSink, legacy_progress_for,
    },
    model::{
        AttentionReason, CommitPhase, ConflictResolution, ManagedArtifacts, NewTransferJob,
        QueueSettings, TRANSFER_HISTORY_LIMIT, TransferEndpoint, TransferJob, TransferJobState,
        TransferPriority, TransferQueueDocument, TransferQueueSnapshot, build_destination_key,
        build_host_key, validate_document_semantics, validate_queue_settings,
    },
    reducer::{
        JobEvent, job_owns_attempt, reduce_job, resolution_abandons_attempt, resolution_is_legal,
    },
    runner::{
        QueueClock, RunnerControl, RunnerControlState, RunnerProgress, RunnerReporter,
        RunnerResult, SharedTransferJobRunner, SystemQueueClock, TransferJobRunner,
    },
    scheduler::{ActiveLease, FailureClass, retry_delay_ms, select_scheduled_jobs_from_document},
    store::{TransferStore, recover_for_startup},
};

const TERMINAL_PERSISTENCE_RETRY_DELAY_MS: u64 = 250;

pub trait QueueStore: Send + Sync {
    fn load(&self) -> Result<TransferQueueDocument, String>;
    fn save(&self, document: &TransferQueueDocument) -> Result<(), String>;
}

impl QueueStore for TransferStore {
    fn load(&self) -> Result<TransferQueueDocument, String> {
        TransferStore::load(self)
            .map(|outcome| outcome.into_document())
            .map_err(|error| error.to_string())
    }

    fn save(&self, document: &TransferQueueDocument) -> Result<(), String> {
        TransferStore::save(self, document).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub struct TransferQueueHandle {
    command_tx: mpsc::UnboundedSender<QueueCommand>,
    snapshot: Arc<RwLock<TransferQueueSnapshot>>,
}

impl TransferQueueHandle {
    pub async fn enqueue(&self, request: NewTransferJob) -> Result<Uuid, String> {
        self.request(|reply| QueueCommand::Enqueue { request, reply })
            .await
    }

    pub fn snapshot(&self) -> TransferQueueSnapshot {
        self.snapshot.read().clone()
    }

    pub async fn pause(&self, id: Uuid) -> Result<(), String> {
        self.request(|reply| QueueCommand::Pause { id, reply })
            .await
    }

    pub async fn resume(&self, id: Uuid) -> Result<(), String> {
        self.request(|reply| QueueCommand::Resume { id, reply })
            .await
    }

    pub async fn pause_all(&self) -> Result<(), String> {
        self.request(|reply| QueueCommand::PauseAll { reply }).await
    }

    pub async fn resume_all(&self) -> Result<(), String> {
        self.request(|reply| QueueCommand::ResumeAll { reply })
            .await
    }

    pub async fn cancel(&self, id: Uuid) -> Result<bool, String> {
        self.request(|reply| QueueCommand::Cancel { id, reply })
            .await
    }

    pub async fn retry(&self, id: Uuid) -> Result<(), String> {
        self.request(|reply| QueueCommand::Retry { id, reply })
            .await
    }

    pub async fn resolve(&self, id: Uuid, resolution: ConflictResolution) -> Result<(), String> {
        self.request(|reply| QueueCommand::Resolve {
            id,
            resolution,
            reply,
        })
        .await
    }

    pub async fn reorder(&self, id: Uuid, before: Option<Uuid>) -> Result<(), String> {
        self.request(|reply| QueueCommand::Reorder { id, before, reply })
            .await
    }

    pub async fn set_priority(&self, id: Uuid, priority: TransferPriority) -> Result<(), String> {
        self.request(|reply| QueueCommand::SetPriority {
            id,
            priority,
            reply,
        })
        .await
    }

    pub async fn clear_completed(&self) -> Result<usize, String> {
        self.request(|reply| QueueCommand::ClearCompleted { reply })
            .await
    }

    pub async fn update_settings(&self, settings: QueueSettings) -> Result<(), String> {
        self.request(|reply| QueueCommand::UpdateSettings { settings, reply })
            .await
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, String>>) -> QueueCommand,
    ) -> Result<T, String> {
        let (reply, response) = oneshot::channel();
        self.command_tx
            .send(command(reply))
            .map_err(|_| "transfer queue actor is unavailable".to_string())?;
        response
            .await
            .map_err(|_| "transfer queue actor dropped its reply".to_string())?
    }
}

pub enum QueueCommand {
    Enqueue {
        request: NewTransferJob,
        reply: oneshot::Sender<Result<Uuid, String>>,
    },
    Pause {
        id: Uuid,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Resume {
        id: Uuid,
        reply: oneshot::Sender<Result<(), String>>,
    },
    PauseAll {
        reply: oneshot::Sender<Result<(), String>>,
    },
    ResumeAll {
        reply: oneshot::Sender<Result<(), String>>,
    },
    Cancel {
        id: Uuid,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    Retry {
        id: Uuid,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Resolve {
        id: Uuid,
        resolution: ConflictResolution,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Reorder {
        id: Uuid,
        before: Option<Uuid>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetPriority {
        id: Uuid,
        priority: TransferPriority,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ClearCompleted {
        reply: oneshot::Sender<Result<usize, String>>,
    },
    UpdateSettings {
        settings: QueueSettings,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// Durable queue state and command receiver prepared before the Tauri builder
/// exists. The actor is constructed and started exactly once in application
/// setup, when its AppHandle-backed event sink and live connection runner are
/// available.
pub struct QueueBootstrap {
    document: TransferQueueDocument,
    store: Arc<dyn QueueStore>,
    command_rx: mpsc::UnboundedReceiver<QueueCommand>,
    snapshot: Arc<RwLock<TransferQueueSnapshot>>,
}

impl QueueBootstrap {
    pub fn start(
        self,
        event_sink: Arc<dyn TransferEventSink>,
        clock: Arc<dyn QueueClock>,
        runner: Arc<dyn TransferJobRunner>,
    ) {
        tauri::async_runtime::spawn(self.into_actor(event_sink, clock, Some(runner)).run());
    }

    fn into_actor(
        self,
        event_sink: Arc<dyn TransferEventSink>,
        clock: Arc<dyn QueueClock>,
        runner: Option<SharedTransferJobRunner>,
    ) -> QueueActor {
        let (runner_event_tx, runner_event_rx) = mpsc::unbounded_channel();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        QueueActor {
            document: self.document,
            store: self.store,
            event_sink,
            clock,
            runner,
            command_rx: self.command_rx,
            runner_event_tx,
            runner_event_rx,
            internal_tx,
            internal_rx,
            active: HashMap::new(),
            pending_cancel_cleanup: HashSet::new(),
            pending_artifact_resolutions: HashMap::new(),
            startup_suspended: true,
            startup_authorized: HashSet::new(),
            pending_terminal_results: HashMap::new(),
            pending_progress: HashMap::new(),
            persistence_fault: false,
            snapshot: self.snapshot,
        }
    }
}

pub struct QueueActor {
    document: TransferQueueDocument,
    store: Arc<dyn QueueStore>,
    event_sink: Arc<dyn TransferEventSink>,
    clock: Arc<dyn QueueClock>,
    runner: Option<SharedTransferJobRunner>,
    command_rx: mpsc::UnboundedReceiver<QueueCommand>,
    runner_event_tx: mpsc::UnboundedSender<RunnerEvent>,
    runner_event_rx: mpsc::UnboundedReceiver<RunnerEvent>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
    internal_rx: mpsc::UnboundedReceiver<InternalEvent>,
    active: HashMap<Uuid, ActiveTask>,
    pending_cancel_cleanup: HashSet<Uuid>,
    pending_artifact_resolutions: HashMap<Uuid, ConflictResolution>,
    /// Runtime-only authorization boundary created by startup recovery. It is
    /// intentionally never persisted: another restart must return to a
    /// no-network state even if one row was explicitly resumed before exit.
    startup_suspended: bool,
    startup_authorized: HashSet<Uuid>,
    pending_terminal_results: HashMap<Uuid, PendingTerminalResult>,
    pending_progress: HashMap<Uuid, PendingProgress>,
    persistence_fault: bool,
    snapshot: Arc<RwLock<TransferQueueSnapshot>>,
}

struct ActiveTask {
    lease_id: Uuid,
    lease: ActiveLease,
    ownership: JobOwnership,
    control_tx: watch::Sender<RunnerControlState>,
    commit_critical: bool,
    deferred_control: Option<RunnerControlState>,
    /// Resume All superseded a pause request that the runner may already have observed.
    resume_after_pause: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum JobOwnership {
    Transfer,
    CancellationCleanup,
    ArtifactResolution(ConflictResolution),
}

struct PendingTerminalResult {
    lease_id: Uuid,
    result: RunnerResult,
    ownership: JobOwnership,
    /// Preserved across terminal-result persistence retries.
    resume_after_pause: bool,
    retry_scheduled: bool,
}

struct PendingProgress {
    lease_id: Uuid,
    bytes: u64,
    speed_bytes_per_second: Option<u64>,
    eta_seconds: Option<u64>,
    deadline_ms: u64,
}

enum InternalEvent {
    RunnerFinished {
        job_id: Uuid,
        lease_id: Uuid,
        result: RunnerResult,
    },
    RetryWake {
        job_id: Uuid,
        attempt: u8,
        deadline_ms: u64,
    },
    ProgressWake {
        job_id: Uuid,
        lease_id: Uuid,
        deadline_ms: u64,
    },
    TerminalPersistenceWake {
        job_id: Uuid,
        lease_id: Uuid,
    },
}

impl QueueActor {
    pub fn bootstrap(
        store: TransferStore,
    ) -> Result<(QueueBootstrap, TransferQueueHandle), String> {
        Self::bootstrap_with_store(Arc::new(store))
    }

    pub fn spawn(
        store: TransferStore,
        event_sink: Arc<dyn TransferEventSink>,
    ) -> Result<TransferQueueHandle, String> {
        Self::spawn_with_services(Arc::new(store), event_sink, Arc::new(SystemQueueClock))
    }

    pub fn spawn_with_services(
        store: Arc<dyn QueueStore>,
        event_sink: Arc<dyn TransferEventSink>,
        clock: Arc<dyn QueueClock>,
    ) -> Result<TransferQueueHandle, String> {
        Self::spawn_with_optional_runner(store, event_sink, clock, None)
    }

    pub fn spawn_with_runner_services(
        store: Arc<dyn QueueStore>,
        event_sink: Arc<dyn TransferEventSink>,
        clock: Arc<dyn QueueClock>,
        runner: Arc<dyn TransferJobRunner>,
    ) -> Result<TransferQueueHandle, String> {
        Self::spawn_with_optional_runner(store, event_sink, clock, Some(runner))
    }

    fn spawn_with_optional_runner(
        store: Arc<dyn QueueStore>,
        event_sink: Arc<dyn TransferEventSink>,
        clock: Arc<dyn QueueClock>,
        runner: Option<SharedTransferJobRunner>,
    ) -> Result<TransferQueueHandle, String> {
        let (bootstrap, handle) = Self::bootstrap_with_store(store)?;
        let actor = bootstrap.into_actor(event_sink, clock, runner);
        tokio::spawn(actor.run());
        Ok(handle)
    }

    fn bootstrap_with_store(
        store: Arc<dyn QueueStore>,
    ) -> Result<(QueueBootstrap, TransferQueueHandle), String> {
        let mut document = store.load()?;
        recover_for_startup(&mut document);
        validate_document_semantics(&document)?;
        store.save(&document)?;
        let snapshot = Arc::new(RwLock::new(TransferQueueSnapshot::from(&document)));
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let bootstrap = QueueBootstrap {
            document,
            store,
            command_rx,
            snapshot: snapshot.clone(),
        };
        let handle = TransferQueueHandle {
            command_tx,
            snapshot,
        };
        Ok((bootstrap, handle))
    }

    async fn run(mut self) {
        self.schedule_restored_retry_wakes();
        loop {
            let should_dispatch = tokio::select! {
                command = self.command_rx.recv() => {
                    let Some(command) = command else { break; };
                    self.handle_command(command).await;
                    true
                }
                event = self.runner_event_rx.recv() => {
                    if let Some(event) = event {
                        self.handle_runner_event(event).await
                    } else {
                        false
                    }
                }
                event = self.internal_rx.recv() => {
                    if let Some(event) = event {
                        self.handle_internal_event(event).await
                    } else {
                        false
                    }
                }
            };
            if should_dispatch {
                self.dispatch_runnable().await;
            }
        }
    }

    async fn handle_command(&mut self, command: QueueCommand) {
        match command {
            QueueCommand::Enqueue { request, reply } => {
                let result = self.enqueue(request).await;
                let _ = reply.send(result);
            }
            QueueCommand::Pause { id, reply } => {
                let result = self.pause(id).await;
                let _ = reply.send(result);
            }
            QueueCommand::Resume { id, reply } => {
                let result = self.resume(id).await;
                let _ = reply.send(result);
            }
            QueueCommand::PauseAll { reply } => {
                let result = self.set_queue_paused(true).await;
                let _ = reply.send(result);
            }
            QueueCommand::ResumeAll { reply } => {
                let result = self.resume_all().await;
                let _ = reply.send(result);
            }
            QueueCommand::Cancel { id, reply } => {
                let result = self.cancel(id).await;
                let _ = reply.send(result);
            }
            QueueCommand::Retry { id, reply } => {
                let result = self.retry(id).await;
                let _ = reply.send(result);
            }
            QueueCommand::Resolve {
                id,
                resolution,
                reply,
            } => {
                let result = self.resolve(id, resolution).await;
                let _ = reply.send(result);
            }
            QueueCommand::Reorder { id, before, reply } => {
                let result = self.reorder(id, before).await;
                let _ = reply.send(result);
            }
            QueueCommand::SetPriority {
                id,
                priority,
                reply,
            } => {
                let result = self.set_priority(id, priority).await;
                let _ = reply.send(result);
            }
            QueueCommand::ClearCompleted { reply } => {
                let result = self.clear_completed().await;
                let _ = reply.send(result);
            }
            QueueCommand::UpdateSettings { settings, reply } => {
                let result = self.update_settings(settings).await;
                let _ = reply.send(result);
            }
        }
    }

    async fn handle_runner_event(&mut self, event: RunnerEvent) -> bool {
        match event {
            RunnerEvent::Checking {
                job_id,
                lease_id,
                ack,
            } => {
                let result = self.checking(job_id, lease_id).await;
                let _ = ack.send(result);
                true
            }
            RunnerEvent::Fingerprinted {
                job_id,
                lease_id,
                fingerprint,
                total_bytes,
                artifacts,
                ack,
            } => {
                let result = self
                    .fingerprinted(job_id, lease_id, fingerprint, total_bytes, artifacts)
                    .await;
                let _ = ack.send(result);
                true
            }
            RunnerEvent::DurableCheckpoint {
                job_id,
                lease_id,
                bytes,
                ack,
            } => {
                let result = self.durable_checkpoint(job_id, lease_id, bytes).await;
                let _ = ack.send(result);
                true
            }
            RunnerEvent::CommitPhase {
                job_id,
                lease_id,
                phase,
                backup_expected,
                ack,
            } => {
                let result = self
                    .commit_phase(job_id, lease_id, phase, backup_expected)
                    .await;
                let _ = ack.send(result);
                true
            }
            RunnerEvent::ProgressReady {
                job_id,
                lease_id,
                slot,
            } => {
                if let Some(progress) = slot.take_latest_and_release_wake() {
                    self.record_progress(job_id, lease_id, progress);
                }
                false
            }
        }
    }

    async fn handle_internal_event(&mut self, event: InternalEvent) -> bool {
        match event {
            InternalEvent::RunnerFinished {
                job_id,
                lease_id,
                result,
            } => {
                let _ = self.runner_finished(job_id, lease_id, result).await;
                true
            }
            InternalEvent::RetryWake {
                job_id,
                attempt,
                deadline_ms,
            } => {
                let _still_waiting = self.document.jobs.iter().any(|job| {
                    job.id == job_id
                        && matches!(
                            job.state,
                            TransferJobState::RetryWaiting {
                                attempt: current_attempt,
                                next_retry_at_ms,
                            } if current_attempt == attempt && next_retry_at_ms == deadline_ms
                        )
                });
                true
            }
            InternalEvent::ProgressWake {
                job_id,
                lease_id,
                deadline_ms,
            } => {
                let _ = self.flush_progress(job_id, lease_id, deadline_ms).await;
                false
            }
            InternalEvent::TerminalPersistenceWake { job_id, lease_id } => {
                let retry = self
                    .pending_terminal_results
                    .get_mut(&job_id)
                    .is_some_and(|pending| {
                        if pending.lease_id != lease_id {
                            return false;
                        }
                        pending.retry_scheduled = false;
                        true
                    });
                if retry {
                    let _ = self.persist_pending_runner_result(job_id, lease_id).await;
                }
                true
            }
        }
    }

    fn schedule_restored_retry_wakes(&self) {
        for job in &self.document.jobs {
            if let TransferJobState::RetryWaiting {
                attempt,
                next_retry_at_ms,
            } = job.state
            {
                self.schedule_retry_wake(job.id, attempt, next_retry_at_ms);
            }
        }
    }

    fn schedule_retry_wake(&self, job_id: Uuid, attempt: u8, deadline_ms: u64) {
        let clock = self.clock.clone();
        let internal_tx = self.internal_tx.clone();
        tokio::spawn(async move {
            clock.sleep_until(deadline_ms).await;
            let _ = internal_tx.send(InternalEvent::RetryWake {
                job_id,
                attempt,
                deadline_ms,
            });
        });
    }

    fn record_progress(&mut self, job_id: Uuid, lease_id: Uuid, progress: RunnerProgress) {
        if self.active_task(job_id, lease_id).is_err() {
            return;
        }
        let deadline_ms = self
            .pending_progress
            .get(&job_id)
            .filter(|progress| progress.lease_id == lease_id)
            .map(|progress| progress.deadline_ms)
            .unwrap_or_else(|| self.clock.now_ms().saturating_add(250));
        let first = self.pending_progress.insert(
            job_id,
            PendingProgress {
                lease_id,
                bytes: progress.bytes,
                speed_bytes_per_second: progress.speed_bytes_per_second,
                eta_seconds: progress.eta_seconds,
                deadline_ms,
            },
        );
        if first.is_none() {
            let clock = self.clock.clone();
            let internal_tx = self.internal_tx.clone();
            tokio::spawn(async move {
                clock.sleep_until(deadline_ms).await;
                let _ = internal_tx.send(InternalEvent::ProgressWake {
                    job_id,
                    lease_id,
                    deadline_ms,
                });
            });
        }
    }

    async fn flush_progress(
        &mut self,
        job_id: Uuid,
        lease_id: Uuid,
        deadline_ms: u64,
    ) -> Result<(), String> {
        let Some(progress) = self.pending_progress.get(&job_id) else {
            return Ok(());
        };
        if progress.lease_id != lease_id || progress.deadline_ms != deadline_ms {
            return Ok(());
        }
        self.active_task(job_id, lease_id)?;
        let progress = self
            .pending_progress
            .remove(&job_id)
            .expect("validated pending progress exists");
        self.apply_job_event(
            job_id,
            JobEvent::Progress {
                bytes: progress.bytes,
                speed_bytes_per_second: progress.speed_bytes_per_second,
                eta_seconds: progress.eta_seconds,
            },
        )
        .await
    }

    async fn dispatch_runnable(&mut self) {
        if self.runner.is_none() || self.persistence_fault {
            return;
        }
        let leases: Vec<_> = self
            .active
            .values()
            .map(|task| task.lease.clone())
            .collect();
        let pending_cleanup: HashSet<_> = self
            .pending_cancel_cleanup
            .iter()
            .copied()
            .chain(self.pending_artifact_resolutions.keys().copied())
            .collect();
        let selected = select_scheduled_jobs_from_document(
            &self.document,
            &leases,
            &pending_cleanup,
            &self.startup_authorized,
            self.clock.now_ms(),
        );
        for id in selected {
            let artifact_resolution = self.pending_artifact_resolutions.get(&id).cloned();
            let start = if self.pending_cancel_cleanup.contains(&id) {
                self.start_cancel_cleanup(id)
            } else if let Some(resolution) = artifact_resolution.clone() {
                self.start_artifact_resolution(id, resolution)
            } else {
                self.start_job(id).await
            };
            if let Err(error) = start {
                self.pending_cancel_cleanup.remove(&id);
                self.pending_artifact_resolutions.remove(&id);
                let event = if artifact_resolution.is_some() {
                    JobEvent::NeedsAttention(AttentionReason::Cleanup { message: error })
                } else {
                    JobEvent::Fail(error)
                };
                let _ = self.apply_job_event(id, event).await;
            } else {
                self.pending_cancel_cleanup.remove(&id);
                self.pending_artifact_resolutions.remove(&id);
            }
        }
    }

    async fn start_job(&mut self, id: Uuid) -> Result<(), String> {
        if self.active.contains_key(&id) {
            return Ok(());
        }

        let now_ms = self.clock.now_ms();
        let mut next = self.document.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| format!("transfer job {id} was not found"))?;
        if matches!(job.state, TransferJobState::RetryWaiting { .. }) {
            *job =
                reduce_job(job, JobEvent::RetryReady, now_ms).map_err(|error| error.to_string())?;
        }
        *job =
            reduce_job(job, JobEvent::BeginConnect, now_ms).map_err(|error| error.to_string())?;
        if job.retry_attempt == 0 {
            job.retry_attempt = 1;
        }
        self.commit(next).await?;
        let job = self
            .document
            .jobs
            .iter()
            .find(|job| job.id == id)
            .expect("started job remains in committed document")
            .clone();

        self.spawn_runner(job, RunnerControlState::Run, JobOwnership::Transfer)
    }

    fn spawn_runner(
        &mut self,
        job: TransferJob,
        initial_control: RunnerControlState,
        ownership: JobOwnership,
    ) -> Result<(), String> {
        let id = job.id;
        if self.active.contains_key(&id) {
            return Ok(());
        }
        let runner = self
            .runner
            .clone()
            .ok_or_else(|| "transfer runner is unavailable".to_string())?;
        let lease = ActiveLease {
            job_id: id,
            host_key: job.host_key.clone(),
            destination_key: job.destination_key.clone(),
        };

        let lease_id = Uuid::new_v4();
        let (control_tx, control_rx) = watch::channel(initial_control);
        self.active.insert(
            id,
            ActiveTask {
                lease_id,
                lease,
                ownership,
                control_tx,
                commit_critical: false,
                deferred_control: None,
                resume_after_pause: false,
            },
        );

        let reporter = RunnerReporter::new(id, lease_id, self.runner_event_tx.clone());
        let internal_tx = self.internal_tx.clone();
        let runner_task = tokio::spawn(async move {
            runner
                .run(job, RunnerControl::new(control_rx), reporter)
                .await
        });
        tokio::spawn(async move {
            let result = runner_task
                .await
                .unwrap_or_else(|error| RunnerResult::Failed {
                    class: FailureClass::Permanent,
                    message: if error.is_panic() {
                        "transfer runner task panicked".into()
                    } else {
                        "transfer runner task failed before reporting a result".into()
                    },
                });
            let _ = internal_tx.send(InternalEvent::RunnerFinished {
                job_id: id,
                lease_id,
                result,
            });
        });
        Ok(())
    }

    fn start_cancel_cleanup(&mut self, id: Uuid) -> Result<(), String> {
        let job = self
            .document
            .jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| format!("transfer job {id} was not found"))?
            .clone();
        self.spawn_runner(
            job,
            RunnerControlState::Cancel,
            JobOwnership::CancellationCleanup,
        )
    }

    fn start_artifact_resolution(
        &mut self,
        id: Uuid,
        resolution: ConflictResolution,
    ) -> Result<(), String> {
        let job = self
            .document
            .jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| format!("transfer job {id} was not found"))?
            .clone();
        self.spawn_runner(
            job,
            RunnerControlState::Cancel,
            JobOwnership::ArtifactResolution(resolution),
        )
    }

    fn active_task(&self, job_id: Uuid, lease_id: Uuid) -> Result<&ActiveTask, String> {
        self.active
            .get(&job_id)
            .filter(|task| task.lease_id == lease_id)
            .ok_or_else(|| format!("stale runner lease for transfer job {job_id}"))
    }

    async fn fingerprinted(
        &mut self,
        job_id: Uuid,
        lease_id: Uuid,
        fingerprint: SourceFingerprint,
        total_bytes: u64,
        artifacts: ManagedArtifacts,
    ) -> Result<(), String> {
        self.active_task(job_id, lease_id)?;
        let now_ms = self.clock.now_ms();
        let mut next = self.document.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == job_id)
            .ok_or_else(|| format!("transfer job {job_id} was not found"))?;
        if matches!(job.state, TransferJobState::Connecting) {
            *job =
                reduce_job(job, JobEvent::BeginCheck, now_ms).map_err(|error| error.to_string())?;
        }
        *job = reduce_job(
            job,
            JobEvent::BeginRun {
                fingerprint,
                total_bytes,
                artifacts,
            },
            now_ms,
        )
        .map_err(|error| error.to_string())?;
        self.commit(next).await
    }

    async fn checking(&mut self, job_id: Uuid, lease_id: Uuid) -> Result<(), String> {
        self.active_task(job_id, lease_id)?;
        self.apply_job_event(job_id, JobEvent::BeginCheck).await
    }

    async fn durable_checkpoint(
        &mut self,
        job_id: Uuid,
        lease_id: Uuid,
        bytes: u64,
    ) -> Result<(), String> {
        self.active_task(job_id, lease_id)?;
        self.apply_job_event(job_id, JobEvent::Checkpoint { bytes })
            .await
    }

    async fn commit_phase(
        &mut self,
        job_id: Uuid,
        lease_id: Uuid,
        phase: CommitPhase,
        backup_expected: Option<bool>,
    ) -> Result<(), String> {
        self.active_task(job_id, lease_id)?;
        let critical = matches!(
            phase,
            CommitPhase::Prepared
                | CommitPhase::BackupMoved
                | CommitPhase::PartialPromoted
                | CommitPhase::CleanupPending
        );
        let control_rollback = if critical {
            let task = self
                .active
                .get_mut(&job_id)
                .expect("validated active task exists");
            let visible = *task.control_tx.borrow();
            let previous = (task.commit_critical, task.deferred_control, visible);
            let deferred = task.deferred_control.unwrap_or(RunnerControlState::Run);
            let pending = strongest_control(deferred, visible);
            task.deferred_control = (pending != RunnerControlState::Run).then_some(pending);
            task.control_tx.send_replace(RunnerControlState::Run);
            task.commit_critical = true;
            Some(previous)
        } else {
            None
        };

        let mut next = self.document.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == job_id)
            .ok_or_else(|| format!("transfer job {job_id} was not found"))?;
        job.commit_phase = phase;
        if let Some(backup_expected) = backup_expected {
            job.commit_backup_expected = Some(backup_expected);
        }
        job.updated_at_ms = self.clock.now_ms();
        if let Err(error) = self.commit(next).await {
            if let Some((was_critical, deferred, visible)) = control_rollback {
                let task = self
                    .active
                    .get_mut(&job_id)
                    .expect("active task remains while commit persistence fails");
                task.commit_critical = was_critical;
                task.deferred_control = deferred;
                task.control_tx.send_replace(visible);
            }
            return Err(error);
        }

        if let Some(task) = self.active.get_mut(&job_id) {
            task.commit_critical = critical;
            if !critical {
                if let Some(control) = task.deferred_control.take() {
                    task.control_tx.send_replace(control);
                }
            }
        }
        Ok(())
    }

    async fn runner_finished(
        &mut self,
        job_id: Uuid,
        lease_id: Uuid,
        result: RunnerResult,
    ) -> Result<(), String> {
        self.active_task(job_id, lease_id)?;
        if self.pending_terminal_results.contains_key(&job_id) {
            return Err(format!(
                "transfer job {job_id} already has a pending runner result"
            ));
        }
        let task = self
            .active
            .remove(&job_id)
            .expect("validated active task exists");
        self.pending_progress.remove(&job_id);
        self.pending_terminal_results.insert(
            job_id,
            PendingTerminalResult {
                lease_id,
                result,
                ownership: task.ownership,
                resume_after_pause: task.resume_after_pause,
                retry_scheduled: false,
            },
        );
        self.persist_pending_runner_result(job_id, lease_id).await
    }

    async fn persist_pending_runner_result(
        &mut self,
        job_id: Uuid,
        lease_id: Uuid,
    ) -> Result<(), String> {
        let (result, ownership, resume_after_pause) = self
            .pending_terminal_results
            .get(&job_id)
            .filter(|pending| pending.lease_id == lease_id)
            .map(|pending| {
                (
                    pending.result.clone(),
                    pending.ownership.clone(),
                    pending.resume_after_pause,
                )
            })
            .ok_or_else(|| format!("transfer job {job_id} has no pending runner result"))?;
        let now_ms = self.clock.now_ms();
        let continue_cancel_cleanup = ownership == JobOwnership::CancellationCleanup
            && matches!(&result, RunnerResult::Paused { .. });
        let mut next = self.document.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == job_id)
            .ok_or_else(|| format!("transfer job {job_id} was not found"))?;

        let application = if let JobOwnership::ArtifactResolution(resolution) = &ownership {
            match &result {
                RunnerResult::Cancelled {
                    cleanup_error: None,
                } => reduce_job(
                    job,
                    JobEvent::ResolveAfterCleanup(resolution.clone()),
                    now_ms,
                )
                .map(|reduced| *job = reduced)
                .map_err(|error| error.to_string()),
                RunnerResult::Cancelled {
                    cleanup_error: Some(message),
                } => reduce_job(
                    job,
                    JobEvent::NeedsAttention(AttentionReason::Cleanup {
                        message: message.clone(),
                    }),
                    now_ms,
                )
                .map(|reduced| *job = reduced)
                .map_err(|error| error.to_string()),
                RunnerResult::NeedsAttention(reason) => {
                    reduce_job(job, JobEvent::NeedsAttention(reason.clone()), now_ms)
                        .map(|reduced| *job = reduced)
                        .map_err(|error| error.to_string())
                }
                _ => Err("artifact cleanup returned an invalid runner result".into()),
            }
        } else {
            match &result {
                RunnerResult::Completed(result) => {
                    job.state = TransferJobState::Completed { result: *result };
                    job.commit_phase = CommitPhase::Complete;
                    job.finished_at_ms = Some(now_ms);
                    job.speed_bytes_per_second = 0;
                    job.eta_seconds = None;
                    job.updated_at_ms = now_ms;
                    Ok(())
                }
                RunnerResult::Paused { durable_checkpoint } => {
                    let mut paused = job.clone();
                    paused.durable_checkpoint = *durable_checkpoint;
                    paused.bytes_transferred = paused.bytes_transferred.max(*durable_checkpoint);
                    reduce_job(&paused, JobEvent::Pause, now_ms)
                        .and_then(|paused| {
                            if resume_after_pause
                                && ownership == JobOwnership::Transfer
                                && !self.document.queue_paused
                            {
                                reduce_job(&paused, JobEvent::Resume, now_ms)
                            } else {
                                Ok(paused)
                            }
                        })
                        .map(|reduced| *job = reduced)
                        .map_err(|error| error.to_string())
                }
                RunnerResult::Cancelled { cleanup_error } => {
                    reduce_job(job, JobEvent::Cancel(cleanup_error.clone()), now_ms)
                        .map(|reduced| *job = reduced)
                        .map_err(|error| error.to_string())
                }
                RunnerResult::NeedsConnection(message) => {
                    reduce_job(job, JobEvent::NeedsConnection(message.clone()), now_ms)
                        .map(|reduced| *job = reduced)
                        .map_err(|error| error.to_string())
                }
                RunnerResult::NeedsAttention(reason) => {
                    reduce_job(job, JobEvent::NeedsAttention(reason.clone()), now_ms)
                        .map(|reduced| *job = reduced)
                        .map_err(|error| error.to_string())
                }
                RunnerResult::Failed { class, message } => {
                    let attempt = job.retry_attempt.max(1);
                    if *class == FailureClass::Transient && attempt < job.max_attempts.min(3) {
                        let next_attempt = attempt + 1;
                        let delay = retry_delay_ms(next_attempt).ok_or_else(|| {
                            format!("missing retry delay for attempt {next_attempt}")
                        })?;
                        reduce_job(
                            job,
                            JobEvent::RetryScheduled {
                                attempt: next_attempt,
                                next_retry_at_ms: now_ms.saturating_add(delay),
                            },
                            now_ms,
                        )
                        .map(|reduced| *job = reduced)
                        .map_err(|error| error.to_string())
                    } else {
                        reduce_job(job, JobEvent::Fail(message.clone()), now_ms)
                            .map(|reduced| *job = reduced)
                            .map_err(|error| error.to_string())
                    }
                }
            }
        };
        let application_error = application
            .err()
            .map(|error| format!("runner result for transfer job {job_id} was rejected: {error}"));
        if let Some(message) = &application_error {
            match reduce_job(
                job,
                JobEvent::NeedsAttention(AttentionReason::Cleanup {
                    message: message.clone(),
                }),
                now_ms,
            ) {
                Ok(quarantined) => *job = quarantined,
                Err(quarantine_error) => {
                    self.pending_terminal_results.remove(&job_id);
                    self.persistence_fault = !self.pending_terminal_results.is_empty();
                    return Err(format!(
                        "{message}; cleanup quarantine was rejected: {quarantine_error}"
                    ));
                }
            }
        }

        if let Err(error) = self.commit(next).await {
            self.persistence_fault = true;
            self.schedule_terminal_persistence_retry(job_id, lease_id);
            return Err(error);
        }
        self.pending_terminal_results.remove(&job_id);
        self.persistence_fault = !self.pending_terminal_results.is_empty();
        if continue_cancel_cleanup && application_error.is_none() {
            self.pending_cancel_cleanup.insert(job_id);
        }
        if matches!(ownership, JobOwnership::ArtifactResolution(_))
            && self.startup_suspended
            && self.document.queue_paused
            && self
                .document
                .jobs
                .iter()
                .find(|job| job.id == job_id)
                .is_some_and(|job| matches!(job.state, TransferJobState::Queued))
        {
            self.startup_authorized.insert(job_id);
        }
        if let Some(job) = self.document.jobs.iter().find(|job| job.id == job_id) {
            if let TransferJobState::RetryWaiting {
                attempt,
                next_retry_at_ms,
            } = job.state
            {
                self.schedule_retry_wake(job_id, attempt, next_retry_at_ms);
            }
        }
        application_error.map_or(Ok(()), Err)
    }

    fn schedule_terminal_persistence_retry(&mut self, job_id: Uuid, lease_id: Uuid) {
        let Some(task) = self
            .pending_terminal_results
            .get_mut(&job_id)
            .filter(|pending| pending.lease_id == lease_id)
        else {
            return;
        };
        if task.retry_scheduled {
            return;
        }
        task.retry_scheduled = true;
        let deadline_ms = self
            .clock
            .now_ms()
            .saturating_add(TERMINAL_PERSISTENCE_RETRY_DELAY_MS);
        let clock = self.clock.clone();
        let internal_tx = self.internal_tx.clone();
        tokio::spawn(async move {
            clock.sleep_until(deadline_ms).await;
            let _ = internal_tx.send(InternalEvent::TerminalPersistenceWake { job_id, lease_id });
        });
    }

    async fn pause(&mut self, id: Uuid) -> Result<(), String> {
        if self.cancellation_cleanup_owns(id) {
            return Ok(());
        }
        self.ensure_lifecycle_mutation_allowed(id)?;
        if self.active.contains_key(&id) {
            self.request_runner_control(id, RunnerControlState::Pause)?;
            Ok(())
        } else {
            self.apply_job_event(id, JobEvent::Pause).await
        }
    }

    fn request_runner_control(
        &mut self,
        id: Uuid,
        requested: RunnerControlState,
    ) -> Result<(), String> {
        let task = self
            .active
            .get_mut(&id)
            .ok_or_else(|| format!("active transfer job {id} was not found"))?;
        if matches!(
            requested,
            RunnerControlState::Pause | RunnerControlState::Cancel
        ) {
            task.resume_after_pause = false;
        }
        if task.commit_critical {
            let deferred = task.deferred_control.unwrap_or(RunnerControlState::Run);
            task.deferred_control = Some(strongest_control(deferred, requested));
        } else {
            let current = *task.control_tx.borrow();
            task.control_tx
                .send_replace(strongest_control(current, requested));
        }
        Ok(())
    }

    fn resume_active_pause(&mut self, id: Uuid) {
        let Some(task) = self.active.get_mut(&id) else {
            return;
        };
        let pending = if task.commit_critical {
            task.deferred_control.unwrap_or(RunnerControlState::Run)
        } else {
            *task.control_tx.borrow()
        };
        if pending != RunnerControlState::Pause {
            return;
        }
        task.resume_after_pause = true;
        if task.commit_critical {
            task.deferred_control = None;
        } else {
            task.control_tx.send_replace(RunnerControlState::Run);
        }
    }

    async fn enqueue(&mut self, request: NewTransferJob) -> Result<Uuid, String> {
        validate_new_job(&request)?;
        if self.document.jobs.iter().any(|job| job.id == request.id) {
            return Err(format!("transfer job {} already exists", request.id));
        }

        let now_ms = self.clock.now_ms();
        let queue_order = self
            .document
            .jobs
            .iter()
            .map(|job| job.queue_order)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "transfer queue order is exhausted".to_string())?;
        let NewTransferJob {
            id,
            protocol,
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
            conflict_policy,
        } = request;
        let job = TransferJob {
            id,
            protocol,
            direction,
            origin,
            endpoint,
            local_path,
            remote_path,
            file_name,
            batch_id,
            priority,
            queue_order,
            host_key,
            destination_key,
            state: TransferJobState::Queued,
            source_fingerprint: None,
            durable_checkpoint: 0,
            bytes_transferred: 0,
            total_bytes: 0,
            speed_bytes_per_second: 0,
            eta_seconds: None,
            retry_attempt: 0,
            max_attempts: 3,
            conflict_policy,
            artifacts: None,
            commit_phase: CommitPhase::None,
            commit_backup_expected: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            started_at_ms: None,
            finished_at_ms: None,
        };

        let mut next = self.document.clone();
        next.jobs.push(job);
        self.commit(next).await?;
        if self.startup_suspended && self.document.queue_paused {
            self.startup_authorized.insert(id);
        }
        Ok(id)
    }

    async fn apply_job_event(&mut self, id: Uuid, event: JobEvent) -> Result<(), String> {
        let mut next = self.document.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| format!("transfer job {id} was not found"))?;
        *job = reduce_job(job, event, self.clock.now_ms()).map_err(|error| error.to_string())?;
        self.commit(next).await
    }

    fn cancellation_cleanup_owns(&self, id: Uuid) -> bool {
        let active_cleanup = self
            .active
            .get(&id)
            .is_some_and(|task| task.ownership == JobOwnership::CancellationCleanup);
        let terminal_cleanup = self
            .pending_terminal_results
            .get(&id)
            .is_some_and(|pending| pending.ownership == JobOwnership::CancellationCleanup);
        self.pending_cancel_cleanup.contains(&id) || active_cleanup || terminal_cleanup
    }

    fn artifact_resolution_owns(&self, id: Uuid) -> bool {
        let active_cleanup = self
            .active
            .get(&id)
            .is_some_and(|task| matches!(&task.ownership, JobOwnership::ArtifactResolution(_)));
        let terminal_cleanup = self
            .pending_terminal_results
            .get(&id)
            .is_some_and(|pending| {
                matches!(&pending.ownership, JobOwnership::ArtifactResolution(_))
            });
        self.pending_artifact_resolutions.contains_key(&id) || active_cleanup || terminal_cleanup
    }

    fn ensure_lifecycle_mutation_allowed(&self, id: Uuid) -> Result<(), String> {
        if self.cancellation_cleanup_owns(id) {
            Err(format!(
                "cancellation cleanup owns transfer job {id} until runner acknowledgement"
            ))
        } else if self.artifact_resolution_owns(id) {
            Err(format!(
                "artifact resolution cleanup owns transfer job {id} until runner acknowledgement"
            ))
        } else {
            Ok(())
        }
    }

    async fn set_queue_paused(&mut self, paused: bool) -> Result<(), String> {
        if self.document.queue_paused != paused {
            let mut next = self.document.clone();
            next.queue_paused = paused;
            self.commit(next).await?;
        }
        if paused {
            self.startup_suspended = false;
            self.startup_authorized.clear();
            let active_ids: Vec<_> = self.active.keys().copied().collect();
            for id in active_ids {
                self.request_runner_control(id, RunnerControlState::Pause)?;
            }
        }
        Ok(())
    }

    async fn resume(&mut self, id: Uuid) -> Result<(), String> {
        self.ensure_lifecycle_mutation_allowed(id)?;
        self.apply_job_event(id, JobEvent::Resume).await?;
        if self.startup_suspended && self.document.queue_paused {
            self.startup_authorized.insert(id);
        }
        Ok(())
    }

    async fn retry(&mut self, id: Uuid) -> Result<(), String> {
        self.ensure_lifecycle_mutation_allowed(id)?;
        self.apply_job_event(id, JobEvent::ManualRetry).await?;
        if self.startup_suspended && self.document.queue_paused {
            self.startup_authorized.insert(id);
        }
        Ok(())
    }

    async fn resume_all(&mut self) -> Result<(), String> {
        let now_ms = self.clock.now_ms();
        let mut next = self.document.clone();
        let mut changed = next.queue_paused;
        next.queue_paused = false;
        for job in &mut next.jobs {
            if matches!(job.state, TransferJobState::Paused) {
                *job =
                    reduce_job(job, JobEvent::Resume, now_ms).map_err(|error| error.to_string())?;
                changed = true;
            }
        }
        if changed {
            self.commit(next).await?;
        }
        let active_ids: Vec<_> = self.active.keys().copied().collect();
        for id in active_ids {
            self.resume_active_pause(id);
        }
        self.startup_suspended = false;
        self.startup_authorized.clear();
        Ok(())
    }

    async fn cancel(&mut self, id: Uuid) -> Result<bool, String> {
        if self.cancellation_cleanup_owns(id) {
            return Ok(true);
        }
        self.ensure_lifecycle_mutation_allowed(id)?;
        if self.active.contains_key(&id) {
            self.active
                .get_mut(&id)
                .expect("active transfer was just found")
                .ownership = JobOwnership::CancellationCleanup;
            self.request_runner_control(id, RunnerControlState::Cancel)?;
            return Ok(true);
        }
        let Some(job) = self.document.jobs.iter().find(|job| job.id == id) else {
            return Ok(false);
        };
        if job.state.is_terminal() {
            return Err(format!("terminal transfer job {id} cannot be cancelled"));
        }
        if job.artifacts.is_some()
            || job.durable_checkpoint != 0
            || job.commit_phase != CommitPhase::None
        {
            if self.runner.is_none() {
                return Err("transfer runner is unavailable".into());
            }
            self.pending_cancel_cleanup.insert(id);
            return Ok(true);
        }
        self.apply_job_event(id, JobEvent::Cancel(None)).await?;
        Ok(true)
    }

    async fn resolve(&mut self, id: Uuid, resolution: ConflictResolution) -> Result<(), String> {
        self.ensure_lifecycle_mutation_allowed(id)?;
        if matches!(
            &resolution,
            ConflictResolution::Rename { destination } if destination.trim().is_empty()
        ) {
            return Err("rename destination must not be empty".into());
        }
        let job = self
            .document
            .jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| format!("transfer job {id} was not found"))?;
        let TransferJobState::NeedsAttention { reason } = &job.state else {
            return Err(format!("transfer job {id} is not awaiting a resolution"));
        };
        if !resolution_is_legal(reason, &resolution) {
            return Err("resolution is not legal for this attention reason".into());
        }
        if resolution_abandons_attempt(&resolution) && job_owns_attempt(job) {
            if self.runner.is_none() {
                return Err("transfer runner is unavailable".into());
            }
            self.pending_artifact_resolutions.insert(id, resolution);
            return Ok(());
        }
        self.apply_job_event(id, JobEvent::Resolve(resolution))
            .await?;
        if self.startup_suspended
            && self.document.queue_paused
            && self
                .document
                .jobs
                .iter()
                .find(|job| job.id == id)
                .is_some_and(|job| matches!(job.state, TransferJobState::Queued))
        {
            self.startup_authorized.insert(id);
        }
        Ok(())
    }

    async fn reorder(&mut self, id: Uuid, before: Option<Uuid>) -> Result<(), String> {
        self.ensure_lifecycle_mutation_allowed(id)?;
        let mut ordered: Vec<(Uuid, u64, u64)> = self
            .document
            .jobs
            .iter()
            .filter(|job| matches!(job.state, TransferJobState::Queued))
            .map(|job| (job.id, job.queue_order, job.created_at_ms))
            .collect();
        ordered.sort_by_key(|(_, order, created)| (*order, *created));
        let queue_order_slots: Vec<_> = ordered.iter().map(|(_, order, _)| *order).collect();

        let source = ordered
            .iter()
            .position(|(job_id, _, _)| *job_id == id)
            .ok_or_else(|| format!("queued transfer job {id} was not found"))?;
        let moved = ordered.remove(source);
        let target = match before {
            Some(before_id) if before_id == id => source.min(ordered.len()),
            Some(before_id) => ordered
                .iter()
                .position(|(job_id, _, _)| *job_id == before_id)
                .ok_or_else(|| format!("queued transfer job {before_id} was not found"))?,
            None => ordered.len(),
        };
        ordered.insert(target, moved);

        let changed: Vec<_> = ordered
            .iter()
            .zip(queue_order_slots)
            .filter_map(|((job_id, previous_order, _), proposed_order)| {
                (*previous_order != proposed_order).then_some((*job_id, proposed_order))
            })
            .collect();
        for (job_id, _) in &changed {
            self.ensure_lifecycle_mutation_allowed(*job_id)?;
        }
        if changed.is_empty() {
            return Ok(());
        }

        let now_ms = self.clock.now_ms();
        let mut next = self.document.clone();
        for (job_id, order) in &changed {
            let job = next
                .jobs
                .iter_mut()
                .find(|job| job.id == *job_id)
                .expect("changed jobs came from this document");
            job.queue_order = *order;
            job.updated_at_ms = now_ms;
        }
        self.commit(next).await
    }

    async fn set_priority(&mut self, id: Uuid, priority: TransferPriority) -> Result<(), String> {
        self.ensure_lifecycle_mutation_allowed(id)?;
        let mut next = self.document.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == id && matches!(job.state, TransferJobState::Queued))
            .ok_or_else(|| format!("queued transfer job {id} was not found"))?;
        if job.priority == priority {
            return Ok(());
        }
        job.priority = priority;
        job.updated_at_ms = self.clock.now_ms();
        self.commit(next).await
    }

    async fn clear_completed(&mut self) -> Result<usize, String> {
        let mut next = self.document.clone();
        let original_len = next.jobs.len();
        next.jobs.retain(|job| {
            !matches!(
                job.state,
                TransferJobState::Completed { .. } | TransferJobState::Cancelled { .. }
            )
        });
        let removed = original_len - next.jobs.len();
        if removed == 0 {
            return Ok(0);
        }
        self.commit(next).await?;
        Ok(removed)
    }

    async fn update_settings(&mut self, settings: QueueSettings) -> Result<(), String> {
        validate_queue_settings(&settings)?;
        if self.document.settings == settings {
            return Ok(());
        }
        let mut next = self.document.clone();
        next.settings = settings;
        self.commit(next).await
    }

    async fn commit(&mut self, mut next: TransferQueueDocument) -> Result<(), String> {
        compact_history(&mut next);
        validate_document_semantics(&next)?;
        next.revision = self
            .document
            .revision
            .checked_add(1)
            .ok_or_else(|| "transfer queue revision is exhausted".to_string())?;
        self.store.save(&next)?;

        let snapshot = TransferQueueSnapshot::from(&next);
        let delta = queue_delta(&self.document, &next);
        self.document = next;
        *self.snapshot.write() = snapshot.clone();

        self.event_sink.job_updated(delta.clone()).await;
        for job in &delta.upserts {
            self.event_sink
                .legacy_progress(legacy_progress_for(job))
                .await;
        }
        self.event_sink
            .queue_summary(QueueSummaryPayload {
                revision: snapshot.revision,
                summary: snapshot.summary,
            })
            .await;
        Ok(())
    }
}

fn strongest_control(
    current: RunnerControlState,
    requested: RunnerControlState,
) -> RunnerControlState {
    use RunnerControlState::{Cancel, Pause, Run};

    match (current, requested) {
        (Cancel, _) | (_, Cancel) => Cancel,
        (Pause, _) | (_, Pause) => Pause,
        (Run, Run) => Run,
    }
}

fn queue_delta(
    previous: &TransferQueueDocument,
    next: &TransferQueueDocument,
) -> QueueEventPayload {
    let upserts = next
        .jobs
        .iter()
        .filter(|job| {
            previous
                .jobs
                .iter()
                .find(|previous_job| previous_job.id == job.id)
                != Some(*job)
        })
        .cloned()
        .collect();
    let removed_ids = previous
        .jobs
        .iter()
        .filter(|job| !next.jobs.iter().any(|next_job| next_job.id == job.id))
        .map(|job| job.id)
        .collect();

    QueueEventPayload {
        revision: next.revision,
        upserts,
        removed_ids,
        queue_paused: next.queue_paused,
        settings: next.settings.clone(),
    }
}

fn validate_new_job(request: &NewTransferJob) -> Result<(), String> {
    if request.local_path.trim().is_empty() {
        return Err("local path must not be empty".into());
    }
    if request.remote_path.trim().is_empty() {
        return Err("remote path must not be empty".into());
    }
    if request.file_name.trim().is_empty() {
        return Err("file name must not be empty".into());
    }
    match &request.endpoint {
        TransferEndpoint::Configured {
            server_entry_id, ..
        } if server_entry_id.trim().is_empty() => {
            return Err("configured server id must not be empty".into());
        }
        TransferEndpoint::AdHoc {
            host, port, user, ..
        } if host.trim().is_empty() || user.trim().is_empty() || *port == 0 => {
            return Err("ad hoc endpoint must include a host, port, and user".into());
        }
        _ => {}
    }
    let expected_host_key = build_host_key(&request.endpoint);
    if request.host_key != expected_host_key {
        return Err("transfer host key is not canonical".into());
    }
    let expected_destination_key = build_destination_key(
        &expected_host_key,
        &request.direction,
        &request.local_path,
        &request.remote_path,
    );
    if request.destination_key != expected_destination_key {
        return Err("transfer destination key is not canonical".into());
    }
    Ok(())
}

fn compact_history(document: &mut TransferQueueDocument) {
    let terminal_count = document
        .jobs
        .iter()
        .filter(|job| job.state.is_terminal())
        .count();
    if terminal_count <= TRANSFER_HISTORY_LIMIT {
        return;
    }

    let mut terminal_indices: Vec<_> = document
        .jobs
        .iter()
        .enumerate()
        .filter(|(_, job)| job.state.is_terminal())
        .map(|(index, job)| {
            (
                index,
                (
                    job.finished_at_ms.unwrap_or(job.created_at_ms),
                    job.created_at_ms,
                    index,
                ),
            )
        })
        .collect();
    terminal_indices.sort_unstable_by_key(|(_, key)| *key);
    let remove_count = terminal_count - TRANSFER_HISTORY_LIMIT;
    let remove: HashSet<_> = terminal_indices
        .into_iter()
        .take(remove_count)
        .map(|(index, _)| index)
        .collect();
    document.jobs = std::mem::take(&mut document.jobs)
        .into_iter()
        .enumerate()
        .filter_map(|(index, job)| (!remove.contains(&index)).then_some(job))
        .collect();
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, VecDeque},
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
    };

    use async_trait::async_trait;
    use parking_lot::RwLock;
    use termlab_remote::transfer::SourceFingerprint;
    use tokio::sync::{Notify, mpsc, oneshot, watch};
    use uuid::Uuid;

    use super::{QueueActor, QueueClock, QueueStore, TransferQueueHandle};
    use crate::remote::transfer_queue::{
        events::{QueueEventPayload, QueueSummaryPayload, TransferEventSink},
        model::{
            AttentionReason, CommitPhase, CompletionResult, ConflictPolicy, ConflictResolution,
            ManagedArtifacts, NewTransferJob, QueueSettings, TransferDirection, TransferEndpoint,
            TransferJob, TransferJobState, TransferOrigin, TransferPriority, TransferProtocol,
            TransferQueueDocument, TransferQueueSnapshot,
        },
        runner::{
            RunnerControl, RunnerControlState, RunnerReporter, RunnerResult, TransferJobRunner,
        },
        scheduler::FailureClass,
        store::TransferStore,
    };

    #[derive(Default)]
    struct GatedRunner {
        starts: Mutex<Vec<Uuid>>,
        jobs: Mutex<Vec<TransferJob>>,
        gates: Mutex<HashMap<Uuid, VecDeque<oneshot::Receiver<RunnerResult>>>>,
        controls: Mutex<HashMap<Uuid, RunnerControl>>,
        reporters: Mutex<HashMap<Uuid, RunnerReporter>>,
    }

    impl GatedRunner {
        fn gate(&self, id: Uuid) -> oneshot::Sender<RunnerResult> {
            let (release, gate) = oneshot::channel();
            self.gates
                .lock()
                .unwrap()
                .entry(id)
                .or_default()
                .push_back(gate);
            release
        }

        fn starts(&self) -> Vec<Uuid> {
            self.starts.lock().unwrap().clone()
        }

        fn jobs(&self) -> Vec<TransferJob> {
            self.jobs.lock().unwrap().clone()
        }

        fn control_state(&self, id: Uuid) -> RunnerControlState {
            self.controls.lock().unwrap()[&id].state()
        }

        fn reporter(&self, id: Uuid) -> RunnerReporter {
            self.reporters.lock().unwrap()[&id].clone()
        }
    }

    #[async_trait]
    impl TransferJobRunner for GatedRunner {
        async fn run(
            &self,
            job: TransferJob,
            control: RunnerControl,
            reporter: RunnerReporter,
        ) -> RunnerResult {
            self.controls.lock().unwrap().insert(job.id, control);
            self.reporters.lock().unwrap().insert(job.id, reporter);
            self.starts.lock().unwrap().push(job.id);
            self.jobs.lock().unwrap().push(job.clone());
            let gate = {
                let mut gates = self.gates.lock().unwrap();
                gates
                    .get_mut(&job.id)
                    .expect("every fake transfer has a gate")
                    .pop_front()
                    .expect("every fake transfer attempt has a gate")
            };
            gate.await.expect("test releases every started transfer")
        }
    }

    struct PanickingFirstRunner {
        panic_id: Uuid,
        starts: Mutex<Vec<Uuid>>,
        next_gate: Mutex<Option<oneshot::Receiver<RunnerResult>>>,
    }

    #[async_trait]
    impl TransferJobRunner for PanickingFirstRunner {
        async fn run(
            &self,
            job: TransferJob,
            _control: RunnerControl,
            _reporter: RunnerReporter,
        ) -> RunnerResult {
            self.starts.lock().unwrap().push(job.id);
            if job.id == self.panic_id {
                panic!("injected transfer runner panic");
            }
            let gate = self
                .next_gate
                .lock()
                .unwrap()
                .take()
                .expect("second transfer has a gate");
            gate.await.expect("test releases the second transfer")
        }
    }

    async fn wait_for_starts(runner: &GatedRunner, count: usize) {
        for _ in 0..100 {
            if runner.starts().len() == count {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("expected {count} starts, observed {:?}", runner.starts());
    }

    struct FakeClock {
        now_ms: AtomicU64,
        advanced: Notify,
    }

    impl FakeClock {
        fn new(now_ms: u64) -> Self {
            Self {
                now_ms: AtomicU64::new(now_ms),
                advanced: Notify::new(),
            }
        }

        fn advance_to(&self, unix_ms: u64) {
            self.now_ms.store(unix_ms, Ordering::SeqCst);
            self.advanced.notify_waiters();
        }
    }

    #[async_trait]
    impl QueueClock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.now_ms.load(Ordering::SeqCst)
        }

        async fn sleep_until(&self, unix_ms: u64) {
            loop {
                let advanced = self.advanced.notified();
                if self.now_ms() >= unix_ms {
                    return;
                }
                advanced.await;
            }
        }
    }

    #[derive(Default)]
    struct RecordingEventSink {
        deltas: Mutex<Vec<QueueEventPayload>>,
        summaries: Mutex<Vec<QueueSummaryPayload>>,
        legacy_count: Mutex<usize>,
        store_path: Mutex<Option<PathBuf>>,
        latest_delta_revision: AtomicU64,
        persisted_emissions: Mutex<Vec<(&'static str, u64, u64)>>,
    }

    impl RecordingEventSink {
        fn observe_store_at(&self, path: PathBuf) {
            *self.store_path.lock().unwrap() = Some(path);
        }

        fn take_delta_containing(&self, id: Uuid) -> QueueEventPayload {
            let mut deltas = self.deltas.lock().unwrap();
            let index = deltas
                .iter()
                .position(|payload| {
                    payload.upserts.iter().any(|job| job.id == id)
                        || payload.removed_ids.contains(&id)
                })
                .expect("atomic job delta was emitted");
            deltas.remove(index)
        }

        fn take_deltas(&self) -> Vec<QueueEventPayload> {
            std::mem::take(&mut *self.deltas.lock().unwrap())
        }

        fn clear(&self) {
            self.deltas.lock().unwrap().clear();
            self.summaries.lock().unwrap().clear();
            *self.legacy_count.lock().unwrap() = 0;
            self.persisted_emissions.lock().unwrap().clear();
        }

        fn event_count(&self) -> usize {
            self.deltas.lock().unwrap().len()
                + self.summaries.lock().unwrap().len()
                + *self.legacy_count.lock().unwrap()
        }

        fn record_persisted_emission(&self, kind: &'static str, emitted_revision: u64) {
            if let Some(path) = self.store_path.lock().unwrap().clone() {
                let document: TransferQueueDocument =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                self.persisted_emissions.lock().unwrap().push((
                    kind,
                    emitted_revision,
                    document.revision,
                ));
            }
        }
    }

    #[async_trait]
    impl TransferEventSink for RecordingEventSink {
        async fn job_updated(&self, payload: QueueEventPayload) {
            self.record_persisted_emission("delta", payload.revision);
            self.latest_delta_revision
                .store(payload.revision, Ordering::SeqCst);
            self.deltas.lock().unwrap().push(payload);
        }

        async fn queue_summary(&self, payload: QueueSummaryPayload) {
            self.record_persisted_emission("summary", payload.revision);
            self.summaries.lock().unwrap().push(payload);
        }

        async fn legacy_progress(&self, _payload: termlab_remote::transfer::TransferProgress) {
            self.record_persisted_emission(
                "legacy",
                self.latest_delta_revision.load(Ordering::SeqCst),
            );
            *self.legacy_count.lock().unwrap() += 1;
        }
    }

    struct ActorHarness {
        _directory: tempfile::TempDir,
        store: TransferStore,
        handle: TransferQueueHandle,
        events: Arc<RecordingEventSink>,
    }

    impl ActorHarness {
        fn new() -> Self {
            Self::with_document(TransferQueueDocument::default())
        }

        fn with_document(document: TransferQueueDocument) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("transfers.json");
            let store = TransferStore::new(path.clone());
            store.save(&document).unwrap();
            let events = Arc::new(RecordingEventSink::default());
            events.observe_store_at(path);
            let handle = QueueActor::spawn(
                TransferStore::new(directory.path().join("transfers.json")),
                events.clone(),
            )
            .unwrap();
            Self {
                _directory: directory,
                store,
                handle,
                events,
            }
        }

        fn with_runner(
            document: TransferQueueDocument,
            runner: Arc<dyn TransferJobRunner>,
        ) -> Self {
            Self::with_runner_and_clock(document, runner, Arc::new(FixedClock))
        }

        fn with_runner_and_clock(
            document: TransferQueueDocument,
            runner: Arc<dyn TransferJobRunner>,
            clock: Arc<dyn QueueClock>,
        ) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("transfers.json");
            let store = TransferStore::new(path.clone());
            store.save(&document).unwrap();
            let events = Arc::new(RecordingEventSink::default());
            events.observe_store_at(path);
            let handle = QueueActor::spawn_with_runner_services(
                Arc::new(TransferStore::new(directory.path().join("transfers.json"))),
                events.clone(),
                clock,
                runner,
            )
            .unwrap();
            Self {
                _directory: directory,
                store,
                handle,
                events,
            }
        }
    }

    fn sample_new_job() -> NewTransferJob {
        new_job(Uuid::from_u128(0x1234), "report.csv")
    }

    fn new_job(id: Uuid, file_name: &str) -> NewTransferJob {
        NewTransferJob {
            id,
            protocol: TransferProtocol::Sftp,
            direction: TransferDirection::Upload,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: "server-1".into(),
                label: "Production".into(),
            },
            local_path: format!("/tmp/{file_name}"),
            remote_path: format!("/srv/{file_name}"),
            file_name: file_name.into(),
            batch_id: None,
            priority: TransferPriority::Normal,
            host_key: "configured:server-1".into(),
            destination_key: format!("configured:server-1:/srv/{file_name}"),
            conflict_policy: ConflictPolicy::Ask,
        }
    }

    fn stored_job(id: Uuid, state: TransferJobState, queue_order: u64) -> TransferJob {
        TransferJob {
            id,
            protocol: TransferProtocol::Sftp,
            direction: TransferDirection::Upload,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: "server-1".into(),
                label: "Production".into(),
            },
            local_path: format!("/tmp/{id}.bin"),
            remote_path: format!("/srv/{id}.bin"),
            file_name: format!("{id}.bin"),
            batch_id: None,
            priority: TransferPriority::Normal,
            queue_order,
            host_key: "configured:server-1".into(),
            destination_key: format!("configured:server-1:/srv/{id}.bin"),
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

    fn durable_cancel_job(id: Uuid, host: &str, queue_order: u64) -> TransferJob {
        let mut job = on_host_with_destination(
            stored_job(id, TransferJobState::Paused, queue_order),
            host,
            &format!("{host}:/srv/{id}.bin"),
        );
        job.durable_checkpoint = 512;
        job.source_fingerprint = Some(SourceFingerprint {
            size: 1_024,
            modified_token: Some("source-v1".into()),
        });
        job.artifacts = Some(
            ManagedArtifacts::for_destination(job.id, &job.remote_path)
                .expect("stored destination has valid managed paths"),
        );
        job
    }

    fn document_with(jobs: Vec<TransferJob>) -> TransferQueueDocument {
        TransferQueueDocument {
            jobs,
            ..TransferQueueDocument::default()
        }
    }

    fn on_host_with_destination(
        mut job: TransferJob,
        host: &str,
        destination: &str,
    ) -> TransferJob {
        job.host_key = host.into();
        job.destination_key = destination.into();
        job
    }

    struct ClientProjection {
        revision: u64,
        queue_paused: bool,
        settings: QueueSettings,
        jobs: Vec<TransferJob>,
    }

    impl ClientProjection {
        fn from_snapshot(snapshot: &TransferQueueSnapshot) -> Self {
            Self {
                revision: snapshot.revision,
                queue_paused: snapshot.queue_paused,
                settings: snapshot.settings.clone(),
                jobs: snapshot.jobs.clone(),
            }
        }

        fn apply(&mut self, delta: QueueEventPayload) {
            assert_eq!(delta.revision, self.revision + 1);
            self.jobs.retain(|job| !delta.removed_ids.contains(&job.id));
            for upsert in delta.upserts {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == upsert.id) {
                    *job = upsert;
                } else {
                    self.jobs.push(upsert);
                }
            }
            self.revision = delta.revision;
            self.queue_paused = delta.queue_paused;
            self.settings = delta.settings;
        }

        fn assert_matches(&self, snapshot: &TransferQueueSnapshot) {
            assert_eq!(self.revision, snapshot.revision);
            assert_eq!(self.queue_paused, snapshot.queue_paused);
            assert_eq!(self.settings, snapshot.settings);
            assert_eq!(self.jobs, snapshot.jobs);
        }
    }

    fn apply_only_delta(
        client: &mut ClientProjection,
        events: &RecordingEventSink,
    ) -> QueueEventPayload {
        let mut deltas = events.take_deltas();
        assert_eq!(deltas.len(), 1, "one atomic delta per committed revision");
        let delta = deltas.remove(0);
        client.apply(delta.clone());
        delta
    }

    async fn wait_for_job(
        handle: &TransferQueueHandle,
        id: Uuid,
        predicate: impl Fn(&TransferJob) -> bool,
    ) -> TransferJob {
        for _ in 0..100 {
            let snapshot = handle.snapshot();
            let job = snapshot.jobs.iter().find(|job| job.id == id).unwrap();
            if predicate(job) {
                return job.clone();
            }
            tokio::task::yield_now().await;
        }
        panic!("transfer job {id} did not reach expected state")
    }

    #[tokio::test]
    async fn restored_pause_starts_nothing_until_explicit_resume_all() {
        let id = Uuid::from_u128(8_001);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());

        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(runner.starts().is_empty());
        harness.handle.pause_all().await.unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(runner.starts().is_empty());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        assert_eq!(runner.starts(), [id]);
        release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn resume_all_requeues_every_recovered_paused_job_and_releases_startup_suspension() {
        let first = Uuid::from_u128(8_003);
        let second = Uuid::from_u128(8_004);
        let document = document_with(vec![
            stored_job(first, TransferJobState::Paused, 1),
            stored_job(second, TransferJobState::Paused, 2),
        ]);
        let runner = Arc::new(GatedRunner::default());
        let first_release = runner.gate(first);
        let second_release = runner.gate(second);
        let harness = ActorHarness::with_runner(document, runner.clone());

        assert!(harness.handle.snapshot().queue_paused);
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 2).await;

        assert!(!harness.handle.snapshot().queue_paused);
        assert_eq!(runner.starts(), [first, second]);
        first_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        second_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn row_resume_authorizes_only_that_recovered_job_during_startup_suspension() {
        let resumed = Uuid::from_u128(8_005);
        let still_paused = Uuid::from_u128(8_006);
        let document = document_with(vec![
            stored_job(resumed, TransferJobState::Paused, 1),
            stored_job(still_paused, TransferJobState::Paused, 2),
        ]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(resumed);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume(resumed).await.unwrap();
        wait_for_starts(&runner, 1).await;

        let snapshot = harness.handle.snapshot();
        assert!(snapshot.queue_paused);
        assert_eq!(runner.starts(), [resumed]);
        assert!(matches!(
            snapshot
                .jobs
                .iter()
                .find(|job| job.id == still_paused)
                .unwrap()
                .state,
            TransferJobState::Paused
        ));
        release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn enqueue_is_explicit_authorization_during_startup_suspension() {
        let restored = Uuid::from_u128(8_007);
        let fresh = Uuid::from_u128(8_008);
        let document = document_with(vec![stored_job(restored, TransferJobState::Paused, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(fresh);
        let harness = ActorHarness::with_runner(document, runner.clone());
        let mut request = new_job(fresh, "editor-save.bin");
        request.origin = TransferOrigin::Editor;
        request.priority = TransferPriority::Interactive;

        harness.handle.enqueue(request).await.unwrap();
        wait_for_starts(&runner, 1).await;

        let snapshot = harness.handle.snapshot();
        assert!(snapshot.queue_paused);
        assert_eq!(runner.starts(), [fresh]);
        assert!(matches!(
            snapshot
                .jobs
                .iter()
                .find(|job| job.id == restored)
                .unwrap()
                .state,
            TransferJobState::Paused
        ));
        release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn explicit_resolution_authorizes_only_that_job_during_startup_suspension() {
        let missing = Uuid::from_u128(8_010);
        let unrelated = Uuid::from_u128(8_011);
        let document = document_with(vec![
            stored_job(
                missing,
                TransferJobState::NeedsAttention {
                    reason: AttentionReason::SourceMissing,
                },
                1,
            ),
            stored_job(unrelated, TransferJobState::Paused, 2),
        ]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(missing);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness
            .handle
            .resolve(missing, ConflictResolution::Restart)
            .await
            .unwrap();
        wait_for_starts(&runner, 1).await;

        assert_eq!(runner.starts(), [missing]);
        assert_eq!(
            harness
                .handle
                .snapshot()
                .jobs
                .iter()
                .find(|job| job.id == unrelated)
                .unwrap()
                .state,
            TransferJobState::Paused
        );
        release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn retry_authorizes_only_that_restored_job_during_startup_suspension() {
        let configured = Uuid::from_u128(8_012);
        let ad_hoc = Uuid::from_u128(8_013);
        let unrelated = Uuid::from_u128(8_014);
        let configured_job = stored_job(
            configured,
            TransferJobState::NeedsConnection {
                message: "reconnect configured server".into(),
            },
            1,
        );
        let mut ad_hoc_job = stored_job(
            ad_hoc,
            TransferJobState::Failed {
                error: "retry ad-hoc server".into(),
            },
            2,
        );
        ad_hoc_job.endpoint = TransferEndpoint::AdHoc {
            host: "example.test".into(),
            port: 22,
            user: "tester".into(),
            proxy_command: None,
            proxy_jump: None,
        };
        ad_hoc_job.host_key = "adhoc:tester@example.test:22".into();
        ad_hoc_job.destination_key = "adhoc:tester@example.test:22:/srv/ad-hoc.bin".into();
        let document = document_with(vec![
            configured_job,
            ad_hoc_job,
            stored_job(unrelated, TransferJobState::Paused, 3),
        ]);
        let runner = Arc::new(GatedRunner::default());
        let configured_release = runner.gate(configured);
        let ad_hoc_release = runner.gate(ad_hoc);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.retry(configured).await.unwrap();
        harness.handle.retry(ad_hoc).await.unwrap();
        wait_for_starts(&runner, 2).await;

        assert_eq!(runner.starts(), [configured, ad_hoc]);
        assert!(harness.handle.snapshot().queue_paused);
        assert_eq!(
            harness
                .handle
                .snapshot()
                .jobs
                .iter()
                .find(|job| job.id == unrelated)
                .unwrap()
                .state,
            TransferJobState::Paused
        );
        configured_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        ad_hoc_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn cross_host_downloads_to_one_local_destination_never_overlap() {
        let first = Uuid::from_u128(8_009);
        let second = Uuid::from_u128(8_010);
        let mut first_job = stored_job(first, TransferJobState::Queued, 1);
        first_job.direction = TransferDirection::Download;
        first_job.host_key = "configured:first".into();
        first_job.endpoint = TransferEndpoint::Configured {
            server_entry_id: "first".into(),
            label: "First".into(),
        };
        first_job.local_path = "/tmp/shared-download.bin".into();
        first_job.destination_key = super::build_destination_key(
            &first_job.host_key,
            &first_job.direction,
            &first_job.local_path,
            &first_job.remote_path,
        );
        let mut second_job = stored_job(second, TransferJobState::Queued, 2);
        second_job.direction = TransferDirection::Download;
        second_job.host_key = "configured:second".into();
        second_job.endpoint = TransferEndpoint::Configured {
            server_entry_id: "second".into(),
            label: "Second".into(),
        };
        second_job.local_path = "/tmp/shared-download.bin".into();
        second_job.destination_key = super::build_destination_key(
            &second_job.host_key,
            &second_job.direction,
            &second_job.local_path,
            &second_job.remote_path,
        );
        let mut document = document_with(vec![first_job, second_job]);
        document.settings = QueueSettings {
            global_limit: 2,
            per_host_limit: 2,
        };
        let runner = Arc::new(GatedRunner::default());
        let first_release = runner.gate(first);
        let second_release = runner.gate(second);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        assert_eq!(runner.starts(), [first]);

        first_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [first, second]);
        second_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn restored_due_retry_stays_suspended_until_explicit_resume() {
        let id = Uuid::from_u128(8_002);
        let mut document = document_with(vec![stored_job(
            id,
            TransferJobState::RetryWaiting {
                attempt: 2,
                next_retry_at_ms: 10,
            },
            1,
        )]);
        document.queue_paused = false;
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let clock = Arc::new(FakeClock::new(20));
        let harness = ActorHarness::with_runner_and_clock(document, runner.clone(), clock);

        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert!(
            runner.starts().is_empty(),
            "startup recovery must not dispatch a due retry"
        );
        assert!(harness.handle.snapshot().queue_paused);
        assert!(matches!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::RetryWaiting { attempt: 2, .. }
        ));

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[test]
    fn bootstrap_surfaces_semantic_store_corruption_without_aborting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("transfers.json");
        let mut document = TransferQueueDocument::default();
        document.settings.global_limit = 0;
        let original = serde_json::to_vec_pretty(&document).unwrap();
        std::fs::write(&path, &original).unwrap();

        let (_bootstrap, handle) = QueueActor::bootstrap(TransferStore::new(path.clone())).unwrap();
        let snapshot = handle.snapshot();

        assert!(snapshot.queue_paused);
        assert!(snapshot.jobs.is_empty());
        assert!(snapshot.recovery_error.is_some());
        let replacement: TransferQueueDocument =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(replacement.queue_paused);
        assert!(replacement.jobs.is_empty());
        assert!(replacement.recovery_error.is_some());
        let quarantined = std::fs::read_dir(directory.path())
            .unwrap()
            .find_map(|entry| {
                let entry = entry.unwrap();
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".corrupt-")
                    .then_some(entry.path())
            })
            .expect("semantic corruption was quarantined");
        assert_eq!(std::fs::read(quarantined).unwrap(), original);
    }

    #[tokio::test]
    async fn runner_panic_is_persisted_and_releases_capacity_for_the_next_job() {
        let panicking = Uuid::from_u128(8_011);
        let next = Uuid::from_u128(8_012);
        let mut document = document_with(vec![
            stored_job(panicking, TransferJobState::Queued, 1),
            stored_job(next, TransferJobState::Queued, 2),
        ]);
        document.settings = QueueSettings {
            global_limit: 1,
            per_host_limit: 1,
        };
        let (release, gate) = oneshot::channel();
        let runner = Arc::new(PanickingFirstRunner {
            panic_id: panicking,
            starts: Mutex::new(Vec::new()),
            next_gate: Mutex::new(Some(gate)),
        });
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        for _ in 0..100 {
            if runner.starts.lock().unwrap().len() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert_eq!(runner.starts.lock().unwrap().as_slice(), [panicking, next]);
        let failed = wait_for_job(&harness.handle, panicking, |job| {
            matches!(job.state, TransferJobState::Failed { .. })
        })
        .await;
        assert!(matches!(
            failed.state,
            TransferJobState::Failed { ref error }
                if error.contains("runner")
                    && error.contains("panic")
                    && !error.contains("injected")
        ));
        assert!(matches!(
            harness
                .store
                .load()
                .unwrap()
                .into_document()
                .jobs
                .iter()
                .find(|job| job.id == panicking)
                .unwrap()
                .state,
            TransferJobState::Failed { .. }
        ));

        release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn dispatch_respects_limits_destination_locks_and_skips_blocked_jobs() {
        let first = Uuid::from_u128(8_101);
        let same_destination = Uuid::from_u128(8_102);
        let same_host = Uuid::from_u128(8_103);
        let other_host = Uuid::from_u128(8_104);
        let waiting = Uuid::from_u128(8_105);
        let mut document = document_with(vec![
            on_host_with_destination(
                stored_job(first, TransferJobState::Queued, 1),
                "host-a",
                "host-a:/same",
            ),
            on_host_with_destination(
                stored_job(same_destination, TransferJobState::Queued, 2),
                "host-b",
                "host-a:/same",
            ),
            on_host_with_destination(
                stored_job(same_host, TransferJobState::Queued, 3),
                "host-a",
                "host-a:/third",
            ),
            on_host_with_destination(
                stored_job(other_host, TransferJobState::Queued, 4),
                "host-c",
                "host-c:/fourth",
            ),
            on_host_with_destination(
                stored_job(waiting, TransferJobState::Queued, 5),
                "host-d",
                "host-d:/fifth",
            ),
        ]);
        document.settings = QueueSettings::default();
        let runner = Arc::new(GatedRunner::default());
        let mut releases: HashMap<_, _> = [first, same_destination, same_host, other_host, waiting]
            .into_iter()
            .map(|id| (id, runner.gate(id)))
            .collect();
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 3).await;
        let starts = runner.starts();
        assert_eq!(starts.len(), 3, "global limit is three");
        assert!(starts.contains(&first));
        assert!(starts.contains(&same_host));
        assert!(
            starts.contains(&other_host),
            "blocked rows do not stop scanning"
        );
        assert!(!starts.contains(&same_destination));
        assert!(!starts.contains(&waiting));

        releases
            .remove(&first)
            .unwrap()
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_starts(&runner, 4).await;
        assert!(runner.starts().contains(&same_destination));
    }

    #[tokio::test]
    async fn completing_one_host_releases_only_that_hosts_capacity() {
        let host_a_first = Uuid::from_u128(8_111);
        let host_a_next = Uuid::from_u128(8_112);
        let host_b = Uuid::from_u128(8_113);
        let mut document = document_with(vec![
            on_host_with_destination(
                stored_job(host_a_first, TransferJobState::Queued, 1),
                "host-a",
                "host-a:/first",
            ),
            on_host_with_destination(
                stored_job(host_a_next, TransferJobState::Queued, 2),
                "host-a",
                "host-a:/next",
            ),
            on_host_with_destination(
                stored_job(host_b, TransferJobState::Queued, 3),
                "host-b",
                "host-b:/only",
            ),
        ]);
        document.settings = QueueSettings {
            global_limit: 2,
            per_host_limit: 1,
        };
        let runner = Arc::new(GatedRunner::default());
        let release_a_first = runner.gate(host_a_first);
        let release_a_next = runner.gate(host_a_next);
        let release_b = runner.gate(host_b);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 2).await;
        assert!(runner.starts().contains(&host_a_first));
        assert!(runner.starts().contains(&host_b));

        release_b
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_job(&harness.handle, host_b, |job| job.state.is_terminal()).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            runner.starts().len(),
            2,
            "host B completion must not free host A's occupied per-host slot"
        );

        release_a_first
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_starts(&runner, 3).await;
        assert_eq!(runner.starts()[2], host_a_next);
        release_a_next
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn interactive_work_is_dispatched_before_earlier_normal_work() {
        let normal = Uuid::from_u128(8_201);
        let interactive = Uuid::from_u128(8_202);
        let mut interactive_job = stored_job(interactive, TransferJobState::Queued, 2);
        interactive_job.priority = TransferPriority::Interactive;
        let mut document = document_with(vec![
            stored_job(normal, TransferJobState::Queued, 1),
            interactive_job,
        ]);
        document.settings.global_limit = 1;
        let runner = Arc::new(GatedRunner::default());
        let _normal_release = runner.gate(normal);
        let _interactive_release = runner.gate(interactive);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        assert_eq!(runner.starts(), [interactive]);
    }

    #[tokio::test]
    async fn progress_is_coalesced_but_checkpoint_ack_waits_for_persistence() {
        let id = Uuid::from_u128(8_301);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let clock = Arc::new(FakeClock::new(1_000));
        let harness = ActorHarness::with_runner_and_clock(document, runner.clone(), clock.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        let reporter = runner.reporter(id);
        reporter.checking().await.unwrap();
        assert!(matches!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Checking
        ));
        reporter
            .fingerprinted(
                SourceFingerprint {
                    size: 8_192,
                    modified_token: Some("source-v1".into()),
                },
                8_192,
                ManagedArtifacts {
                    partial_path: "/tmp/partial".into(),
                    backup_path: "/tmp/backup".into(),
                },
            )
            .await
            .unwrap();
        let revision_before_progress = harness.store.load().unwrap().into_document().revision;
        harness.events.clear();

        reporter.progress(1_024, Some(500), Some(14));
        reporter.progress(2_048, Some(600), Some(10));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            harness.store.load().unwrap().into_document().revision,
            revision_before_progress,
            "display progress does not write for every chunk"
        );
        assert_eq!(harness.events.event_count(), 0);

        clock.advance_to(1_250);
        let displayed =
            wait_for_job(&harness.handle, id, |job| job.bytes_transferred == 2_048).await;
        assert_eq!(displayed.durable_checkpoint, 0);

        reporter.durable_checkpoint(1_536).await.unwrap();
        let persisted = harness.store.load().unwrap().into_document();
        let job = persisted.jobs.iter().find(|job| job.id == id).unwrap();
        assert_eq!(job.durable_checkpoint, 1_536);
        release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn pause_waits_for_runner_checkpoint_and_result_before_publishing_paused() {
        let id = Uuid::from_u128(8_401);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        let reporter = runner.reporter(id);
        reporter
            .fingerprinted(
                SourceFingerprint {
                    size: 4_096,
                    modified_token: Some("pause-source".into()),
                },
                4_096,
                ManagedArtifacts {
                    partial_path: "/tmp/pause.partial".into(),
                    backup_path: "/tmp/pause.backup".into(),
                },
            )
            .await
            .unwrap();

        harness.handle.pause(id).await.unwrap();
        assert_eq!(runner.control_state(id), RunnerControlState::Pause);
        assert!(matches!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Running
        ));

        reporter.durable_checkpoint(4_096).await.unwrap();
        assert!(matches!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Running
        ));
        release
            .send(RunnerResult::Paused {
                durable_checkpoint: 4_096,
            })
            .unwrap();
        let paused = wait_for_job(&harness.handle, id, |job| {
            matches!(job.state, TransferJobState::Paused)
        })
        .await;
        assert_eq!(paused.durable_checkpoint, 4_096);
    }

    #[tokio::test]
    async fn resume_all_supersedes_an_unacknowledged_pause_all() {
        let id = Uuid::from_u128(8_402);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let first_release = runner.gate(id);
        let second_release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        harness.handle.pause_all().await.unwrap();
        assert_eq!(runner.control_state(id), RunnerControlState::Pause);
        harness.handle.resume_all().await.unwrap();
        assert_eq!(runner.control_state(id), RunnerControlState::Run);

        first_release
            .send(RunnerResult::Paused {
                durable_checkpoint: 512,
            })
            .unwrap();
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.control_state(id), RunnerControlState::Run);
        assert_eq!(runner.starts(), [id, id]);
        assert!(!harness.handle.snapshot().queue_paused);

        second_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
    }

    #[tokio::test]
    async fn a_later_pause_supersedes_resume_all_intent() {
        let id = Uuid::from_u128(8_403);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        harness.handle.pause_all().await.unwrap();
        harness.handle.resume_all().await.unwrap();
        harness.handle.pause(id).await.unwrap();
        assert_eq!(runner.control_state(id), RunnerControlState::Pause);

        release
            .send(RunnerResult::Paused {
                durable_checkpoint: 768,
            })
            .unwrap();
        let paused = wait_for_job(&harness.handle, id, |job| {
            matches!(job.state, TransferJobState::Paused)
        })
        .await;
        assert_eq!(paused.durable_checkpoint, 768);
        assert_eq!(runner.starts(), [id]);
    }

    #[tokio::test]
    async fn cancel_after_resume_all_keeps_a_late_paused_result_cancellation_owned() {
        let id = Uuid::from_u128(8_404);
        let mut job = stored_job(id, TransferJobState::Queued, 1);
        job.source_fingerprint = Some(SourceFingerprint {
            size: 1_024,
            modified_token: Some("cancel-after-resume".into()),
        });
        job.durable_checkpoint = 256;
        job.artifacts = Some(ManagedArtifacts {
            partial_path: "/tmp/.cancel-after-resume.partial".into(),
            backup_path: "/tmp/.cancel-after-resume.backup".into(),
        });
        let runner = Arc::new(GatedRunner::default());
        let transfer_release = runner.gate(id);
        let cleanup_release = runner.gate(id);
        let harness = ActorHarness::with_runner(document_with(vec![job]), runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        harness.handle.pause_all().await.unwrap();
        harness.handle.resume_all().await.unwrap();
        assert!(harness.handle.cancel(id).await.unwrap());
        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);

        transfer_release
            .send(RunnerResult::Paused {
                durable_checkpoint: 768,
            })
            .unwrap();
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [id, id]);
        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
        assert_eq!(
            harness.handle.snapshot().jobs[0].durable_checkpoint,
            768,
            "the late pause checkpoint is durable before cleanup starts"
        );

        cleanup_release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
        wait_for_job(&harness.handle, id, |job| {
            matches!(job.state, TransferJobState::Cancelled { .. })
        })
        .await;
    }

    #[tokio::test]
    async fn cancel_remains_visible_when_followed_by_per_job_pause() {
        let id = Uuid::from_u128(8_451);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let _release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        assert!(harness.handle.cancel(id).await.unwrap());
        harness.handle.pause(id).await.unwrap();

        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
    }

    #[tokio::test]
    async fn cancel_remains_visible_when_followed_by_pause_all() {
        let id = Uuid::from_u128(8_452);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let _release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        assert!(harness.handle.cancel(id).await.unwrap());
        harness.handle.pause_all().await.unwrap();

        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
    }

    #[tokio::test]
    async fn cancel_is_deferred_across_commit_swap_until_runner_cleanup_result() {
        let id = Uuid::from_u128(8_501);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        let reporter = runner.reporter(id);

        reporter.commit_prepared(true).await.unwrap();
        assert_eq!(
            harness.handle.snapshot().jobs[0].commit_backup_expected,
            Some(true)
        );
        assert!(harness.handle.cancel(id).await.unwrap());
        assert_eq!(runner.control_state(id), RunnerControlState::Run);
        assert!(matches!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Connecting
        ));

        reporter.commit_phase(CommitPhase::Complete).await.unwrap();
        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
        release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
        wait_for_job(&harness.handle, id, |job| {
            matches!(job.state, TransferJobState::Cancelled { .. })
        })
        .await;
    }

    #[tokio::test]
    async fn prepared_ack_hides_an_already_visible_cancel_until_the_safe_phase() {
        let id = Uuid::from_u128(8_502);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let _release = runner.gate(id);
        let harness = ActorHarness::with_runner(document, runner.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        let reporter = runner.reporter(id);

        assert!(harness.handle.cancel(id).await.unwrap());
        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);

        reporter.commit_phase(CommitPhase::Prepared).await.unwrap();
        assert_eq!(
            runner.control_state(id),
            RunnerControlState::Run,
            "the Prepared persistence barrier hides control during the swap"
        );

        harness.handle.pause_all().await.unwrap();
        assert_eq!(runner.control_state(id), RunnerControlState::Run);
        reporter.commit_phase(CommitPhase::Complete).await.unwrap();
        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
    }

    #[tokio::test]
    async fn transient_failures_persist_one_and_two_second_retries_then_stop() {
        let id = Uuid::from_u128(8_601);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let first = runner.gate(id);
        let second = runner.gate(id);
        let third = runner.gate(id);
        let clock = Arc::new(FakeClock::new(10_000));
        let harness = ActorHarness::with_runner_and_clock(document, runner.clone(), clock.clone());
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        first
            .send(RunnerResult::Failed {
                class: FailureClass::Transient,
                message: "connection reset on attempt one".into(),
            })
            .unwrap();
        let attempt_two = wait_for_job(&harness.handle, id, |job| {
            matches!(
                job.state,
                TransferJobState::RetryWaiting {
                    attempt: 2,
                    next_retry_at_ms: 11_000
                }
            )
        })
        .await;
        assert_eq!(attempt_two.retry_attempt, 2);
        let persisted = harness.store.load().unwrap().into_document();
        assert_eq!(persisted.jobs[0].state, attempt_two.state);

        clock.advance_to(10_999);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(runner.starts().len(), 1);
        clock.advance_to(11_000);
        wait_for_starts(&runner, 2).await;
        second
            .send(RunnerResult::Failed {
                class: FailureClass::Transient,
                message: "timeout on attempt two".into(),
            })
            .unwrap();
        wait_for_job(&harness.handle, id, |job| {
            matches!(
                job.state,
                TransferJobState::RetryWaiting {
                    attempt: 3,
                    next_retry_at_ms: 13_000
                }
            )
        })
        .await;

        harness.handle.pause_all().await.unwrap();
        clock.advance_to(13_000);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            runner.starts().len(),
            2,
            "paused queue suppresses due retry"
        );
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 3).await;
        third
            .send(RunnerResult::Failed {
                class: FailureClass::Transient,
                message: "disconnect on final attempt".into(),
            })
            .unwrap();
        let failed = wait_for_job(&harness.handle, id, |job| {
            matches!(job.state, TransferJobState::Failed { .. })
        })
        .await;
        assert_eq!(failed.retry_attempt, 3);
    }

    #[tokio::test]
    async fn permanent_failure_is_terminal_after_first_attempt() {
        let id = Uuid::from_u128(8_701);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let clock = Arc::new(FakeClock::new(20_000));
        let harness = ActorHarness::with_runner_and_clock(document, runner.clone(), clock);
        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        release
            .send(RunnerResult::Failed {
                class: FailureClass::Permanent,
                message: "permission denied".into(),
            })
            .unwrap();
        let failed = wait_for_job(&harness.handle, id, |job| {
            matches!(job.state, TransferJobState::Failed { .. })
        })
        .await;
        assert_eq!(failed.retry_attempt, 1);
        assert_eq!(runner.starts().len(), 1);
    }

    #[tokio::test]
    async fn enqueue_persists_before_job_event_is_published() {
        let harness = ActorHarness::new();
        let supplied_id = sample_new_job().id;

        let id = harness.handle.enqueue(sample_new_job()).await.unwrap();
        let event = harness.events.take_delta_containing(id);
        let on_disk = harness.store.load().unwrap().into_document();

        assert_eq!(id, supplied_id);
        assert_eq!(on_disk.revision, 1);
        assert_eq!(on_disk.revision, event.revision);
        assert_eq!(event.upserts.len(), 1);
        assert!(event.removed_ids.is_empty());
        assert_eq!(event.queue_paused, on_disk.queue_paused);
        assert_eq!(event.settings, on_disk.settings);
        assert_eq!(
            *harness.events.persisted_emissions.lock().unwrap(),
            [("delta", 1, 1), ("legacy", 1, 1), ("summary", 1, 1),]
        );
        assert!(on_disk.jobs.iter().any(|job| job.id == id));
    }

    #[tokio::test]
    async fn snapshot_matches_the_durable_document() {
        let harness = ActorHarness::new();
        harness.handle.enqueue(sample_new_job()).await.unwrap();

        let snapshot = harness.handle.snapshot();
        let document = harness.store.load().unwrap().into_document();

        assert_eq!(snapshot.revision, document.revision);
        assert_eq!(snapshot.queue_paused, document.queue_paused);
        assert_eq!(snapshot.settings, document.settings);
        assert_eq!(snapshot.jobs, document.jobs);
    }

    #[tokio::test]
    async fn cloned_handles_share_one_actor_snapshot() {
        let harness = ActorHarness::new();
        let second_handle = harness.handle.clone();

        harness.handle.enqueue(sample_new_job()).await.unwrap();

        assert_eq!(second_handle.snapshot(), harness.handle.snapshot());
        assert_eq!(second_handle.snapshot().jobs.len(), 1);
    }

    #[tokio::test]
    async fn enqueue_preserves_order_and_rejects_duplicate_uuid_without_publishing() {
        let harness = ActorHarness::new();
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        harness
            .handle
            .enqueue(new_job(first, "one.bin"))
            .await
            .unwrap();
        harness
            .handle
            .enqueue(new_job(second, "two.bin"))
            .await
            .unwrap();
        let before = harness.handle.snapshot();
        let event_count = harness.events.event_count();

        let error = harness
            .handle
            .enqueue(new_job(first, "duplicate.bin"))
            .await
            .unwrap_err();

        assert!(error.contains("already exists"));
        assert_eq!(harness.handle.snapshot(), before);
        assert_eq!(harness.events.event_count(), event_count);
        assert_eq!(
            before
                .jobs
                .iter()
                .map(|job| (job.id, job.queue_order))
                .collect::<Vec<_>>(),
            [(first, 1), (second, 2)]
        );
    }

    #[tokio::test]
    async fn pause_and_resume_use_legal_job_transitions() {
        let harness = ActorHarness::new();
        let id = harness.handle.enqueue(sample_new_job()).await.unwrap();

        harness.handle.pause(id).await.unwrap();
        assert_eq!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Paused
        );
        harness.handle.resume(id).await.unwrap();
        assert_eq!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Queued
        );
        assert_eq!(harness.handle.snapshot().revision, 3);
    }

    #[tokio::test]
    async fn pause_all_and_resume_all_change_only_global_queue_intent() {
        let harness = ActorHarness::new();
        harness.handle.enqueue(sample_new_job()).await.unwrap();

        harness.handle.resume_all().await.unwrap();
        assert!(!harness.handle.snapshot().queue_paused);
        assert_eq!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Queued
        );
        harness.handle.pause_all().await.unwrap();
        assert!(harness.handle.snapshot().queue_paused);
        assert_eq!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Queued
        );
    }

    #[tokio::test]
    async fn queued_job_without_cleanup_work_can_cancel_immediately() {
        let harness = ActorHarness::new();
        let id = harness.handle.enqueue(sample_new_job()).await.unwrap();

        assert!(harness.handle.cancel(id).await.unwrap());
        assert_eq!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Cancelled {
                cleanup_error: None
            }
        );
        let before = harness.handle.snapshot();
        assert!(!harness.handle.cancel(Uuid::from_u128(99)).await.unwrap());
        assert_eq!(harness.handle.snapshot(), before);
    }

    #[tokio::test]
    async fn retry_moves_failed_and_disconnected_jobs_back_to_queue() {
        for (id, state) in [
            (
                Uuid::from_u128(11),
                TransferJobState::Failed {
                    error: "disk full".into(),
                },
            ),
            (
                Uuid::from_u128(12),
                TransferJobState::NeedsConnection {
                    message: "reconnect".into(),
                },
            ),
        ] {
            let harness =
                ActorHarness::with_document(document_with(vec![stored_job(id, state, 1)]));
            harness.handle.retry(id).await.unwrap();
            assert_eq!(
                harness.handle.snapshot().jobs[0].state,
                TransferJobState::Queued
            );
        }
    }

    #[tokio::test]
    async fn conflict_resolutions_are_validated_and_reduced() {
        let cases = [
            ConflictResolution::Overwrite,
            ConflictResolution::Rename {
                destination: "/srv/renamed.bin".into(),
            },
            ConflictResolution::Skip,
            ConflictResolution::Restart,
        ];
        for (index, resolution) in cases.into_iter().enumerate() {
            let id = Uuid::from_u128(100 + index as u128);
            let state = TransferJobState::NeedsAttention {
                reason: AttentionReason::DestinationConflict {
                    resume_available: false,
                },
            };
            let harness =
                ActorHarness::with_document(document_with(vec![stored_job(id, state, 1)]));
            harness
                .handle
                .resolve(id, resolution.clone())
                .await
                .unwrap();
            let job = &harness.handle.snapshot().jobs[0];
            match resolution {
                ConflictResolution::Skip => assert_eq!(
                    job.state,
                    TransferJobState::Completed {
                        result: CompletionResult::Skipped,
                    }
                ),
                ConflictResolution::Rename { destination } => {
                    assert_eq!(job.remote_path, destination);
                    assert_eq!(job.state, TransferJobState::Queued);
                    assert_eq!(job.conflict_policy, ConflictPolicy::Ask);
                }
                _ => assert_eq!(job.state, TransferJobState::Queued),
            }
        }

        let id = Uuid::from_u128(200);
        let state = TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: false,
            },
        };
        let harness = ActorHarness::with_document(document_with(vec![stored_job(id, state, 1)]));
        let before = harness.handle.snapshot();
        assert!(
            harness
                .handle
                .resolve(id, ConflictResolution::Resume)
                .await
                .is_err()
        );
        assert!(
            harness
                .handle
                .resolve(
                    id,
                    ConflictResolution::Rename {
                        destination: "  ".into()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(harness.handle.snapshot(), before);
        assert_eq!(harness.events.event_count(), 0);
    }

    #[tokio::test]
    async fn resume_resolution_requires_an_available_destination_partial() {
        let id = Uuid::from_u128(201);
        let state = TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: true,
            },
        };
        let harness = ActorHarness::with_document(document_with(vec![stored_job(id, state, 1)]));
        harness
            .handle
            .resolve(id, ConflictResolution::Resume)
            .await
            .unwrap();
        assert_eq!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Queued
        );
    }

    #[tokio::test]
    async fn overwrite_and_resume_redispatch_persisted_authorization_to_the_runner() {
        for (id, resolution, resume) in [
            (Uuid::from_u128(202), ConflictResolution::Overwrite, false),
            (Uuid::from_u128(203), ConflictResolution::Resume, true),
        ] {
            let mut job = stored_job(
                id,
                TransferJobState::NeedsAttention {
                    reason: AttentionReason::DestinationConflict {
                        resume_available: resume,
                    },
                },
                1,
            );
            let expected_fingerprint = SourceFingerprint {
                size: 8_192,
                modified_token: Some("source-v1".into()),
            };
            let expected_artifacts = ManagedArtifacts {
                partial_path: format!("/srv/.{id}.part"),
                backup_path: format!("/srv/.{id}.backup"),
            };
            job.source_fingerprint = Some(expected_fingerprint.clone());
            job.durable_checkpoint = 4_096;
            job.artifacts = Some(expected_artifacts.clone());

            let runner = Arc::new(GatedRunner::default());
            let cleanup_release = (!resume).then(|| runner.gate(id));
            let transfer_release = runner.gate(id);
            let harness = ActorHarness::with_runner(document_with(vec![job]), runner.clone());
            harness.handle.resume_all().await.unwrap();
            harness.handle.resolve(id, resolution).await.unwrap();
            wait_for_starts(&runner, 1).await;

            if let Some(cleanup_release) = cleanup_release {
                let cleanup_job = runner.jobs().pop().unwrap();
                assert_eq!(cleanup_job.conflict_policy, ConflictPolicy::Ask);
                assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
                cleanup_release
                    .send(RunnerResult::Cancelled {
                        cleanup_error: None,
                    })
                    .unwrap();
                wait_for_starts(&runner, 2).await;
            }

            let dispatched = runner.jobs().pop().unwrap();
            assert_eq!(dispatched.conflict_policy, ConflictPolicy::Overwrite);
            if resume {
                assert_eq!(dispatched.source_fingerprint, Some(expected_fingerprint));
                assert_eq!(dispatched.durable_checkpoint, 4_096);
                assert_eq!(dispatched.artifacts, Some(expected_artifacts));
            } else {
                assert_eq!(dispatched.source_fingerprint, None);
                assert_eq!(dispatched.durable_checkpoint, 0);
                assert_eq!(dispatched.artifacts, None);
            }

            transfer_release
                .send(RunnerResult::Completed(CompletionResult::Transferred))
                .unwrap();
        }
    }

    #[tokio::test]
    async fn reorder_and_priority_update_queued_work() {
        let harness = ActorHarness::new();
        let first = Uuid::from_u128(301);
        let second = Uuid::from_u128(302);
        let third = Uuid::from_u128(303);
        harness
            .handle
            .enqueue(new_job(first, "one.bin"))
            .await
            .unwrap();
        harness
            .handle
            .enqueue(new_job(second, "two.bin"))
            .await
            .unwrap();
        harness
            .handle
            .enqueue(new_job(third, "three.bin"))
            .await
            .unwrap();

        harness.handle.reorder(third, Some(first)).await.unwrap();
        harness
            .handle
            .set_priority(second, TransferPriority::Interactive)
            .await
            .unwrap();
        let snapshot = harness.handle.snapshot();
        assert_eq!(
            snapshot
                .jobs
                .iter()
                .map(|job| (job.id, job.queue_order))
                .collect::<Vec<_>>(),
            [(first, 2), (second, 3), (third, 1)]
        );
        assert_eq!(snapshot.jobs[1].priority, TransferPriority::Interactive);
    }

    #[tokio::test]
    async fn reorder_uses_queued_order_slots_without_mutating_running_work() {
        let running_id = Uuid::from_u128(304);
        let first_queued_id = Uuid::from_u128(305);
        let second_queued_id = Uuid::from_u128(306);
        let document = document_with(vec![
            stored_job(running_id, TransferJobState::Running, 1),
            stored_job(first_queued_id, TransferJobState::Queued, 2),
            stored_job(second_queued_id, TransferJobState::Queued, 3),
        ]);
        let events = Arc::new(RecordingEventSink::default());
        let (mut actor, _) = actor_without_startup_recovery(document, events.clone());

        actor
            .reorder(second_queued_id, Some(first_queued_id))
            .await
            .unwrap();

        assert_eq!(actor.document.jobs[0].queue_order, 1);
        assert_eq!(actor.document.jobs[0].updated_at_ms, 10);
        assert_eq!(actor.document.jobs[1].queue_order, 3);
        assert_eq!(actor.document.jobs[2].queue_order, 2);
        let delta = events.take_deltas().pop().unwrap();
        assert_eq!(
            delta.upserts.iter().map(|job| job.id).collect::<Vec<_>>(),
            [first_queued_id, second_queued_id]
        );
    }

    #[tokio::test]
    async fn reorder_emits_one_delta_that_converges_to_the_committed_snapshot() {
        let harness = ActorHarness::new();
        let first = Uuid::from_u128(311);
        let second = Uuid::from_u128(312);
        let third = Uuid::from_u128(313);
        for (id, name) in [(first, "one"), (second, "two"), (third, "three")] {
            harness
                .handle
                .enqueue(new_job(id, &format!("{name}.bin")))
                .await
                .unwrap();
        }
        let mut client = ClientProjection::from_snapshot(&harness.handle.snapshot());
        harness.events.clear();

        harness.handle.reorder(third, Some(first)).await.unwrap();

        let delta = apply_only_delta(&mut client, &harness.events);
        assert_eq!(delta.upserts.len(), 3);
        assert!(delta.removed_ids.is_empty());
        client.assert_matches(&harness.handle.snapshot());
    }

    #[tokio::test]
    async fn single_row_update_emits_one_delta_that_converges() {
        let harness = ActorHarness::new();
        let id = harness.handle.enqueue(sample_new_job()).await.unwrap();
        let mut client = ClientProjection::from_snapshot(&harness.handle.snapshot());
        harness.events.clear();

        harness.handle.pause(id).await.unwrap();

        let delta = apply_only_delta(&mut client, &harness.events);
        assert_eq!(
            delta.upserts.iter().map(|job| job.id).collect::<Vec<_>>(),
            [id]
        );
        assert!(delta.removed_ids.is_empty());
        client.assert_matches(&harness.handle.snapshot());
    }

    #[tokio::test]
    async fn settings_delta_converges_without_any_job_change() {
        let harness = ActorHarness::new();
        let mut client = ClientProjection::from_snapshot(&harness.handle.snapshot());
        harness.events.clear();
        let settings = QueueSettings {
            global_limit: 7,
            per_host_limit: 9,
        };

        harness
            .handle
            .update_settings(settings.clone())
            .await
            .unwrap();

        let delta = apply_only_delta(&mut client, &harness.events);
        assert!(delta.upserts.is_empty());
        assert!(delta.removed_ids.is_empty());
        assert_eq!(delta.settings, settings);
        client.assert_matches(&harness.handle.snapshot());
    }

    #[tokio::test]
    async fn clear_completed_delta_removes_rows_and_converges() {
        let completed_id = Uuid::from_u128(421);
        let failed_id = Uuid::from_u128(422);
        let harness = ActorHarness::with_document(document_with(vec![
            stored_job(
                completed_id,
                TransferJobState::Completed {
                    result: CompletionResult::Transferred,
                },
                1,
            ),
            stored_job(
                failed_id,
                TransferJobState::Failed {
                    error: "retry me".into(),
                },
                2,
            ),
        ]));
        let mut client = ClientProjection::from_snapshot(&harness.handle.snapshot());
        harness.events.clear();

        assert_eq!(harness.handle.clear_completed().await.unwrap(), 1);

        let delta = apply_only_delta(&mut client, &harness.events);
        assert!(delta.upserts.is_empty());
        assert_eq!(delta.removed_ids, [completed_id]);
        client.assert_matches(&harness.handle.snapshot());
    }

    #[tokio::test]
    async fn automatic_compaction_is_included_in_the_same_atomic_delta() {
        let oldest_id = Uuid::from_u128(1_000);
        let mut jobs: Vec<_> = (0..500)
            .map(|index| {
                stored_job(
                    Uuid::from_u128(1_000 + index),
                    TransferJobState::Completed {
                        result: CompletionResult::Transferred,
                    },
                    index as u64 + 1,
                )
            })
            .collect();
        let queued_id = Uuid::from_u128(2_000);
        jobs.push(stored_job(queued_id, TransferJobState::Queued, 501));
        let harness = ActorHarness::with_document(document_with(jobs));
        let mut client = ClientProjection::from_snapshot(&harness.handle.snapshot());
        harness.events.clear();

        assert!(harness.handle.cancel(queued_id).await.unwrap());

        let delta = apply_only_delta(&mut client, &harness.events);
        assert_eq!(delta.removed_ids, [oldest_id]);
        assert_eq!(
            delta.upserts.iter().map(|job| job.id).collect::<Vec<_>>(),
            [queued_id]
        );
        client.assert_matches(&harness.handle.snapshot());
    }

    #[tokio::test]
    async fn clear_completed_keeps_retryable_failures_visible() {
        let active = stored_job(Uuid::from_u128(401), TransferJobState::Paused, 1);
        let completed = stored_job(
            Uuid::from_u128(402),
            TransferJobState::Completed {
                result: CompletionResult::Transferred,
            },
            2,
        );
        let failed = stored_job(
            Uuid::from_u128(403),
            TransferJobState::Failed {
                error: "failed".into(),
            },
            3,
        );
        let cancelled = stored_job(
            Uuid::from_u128(404),
            TransferJobState::Cancelled {
                cleanup_error: None,
            },
            4,
        );
        let harness =
            ActorHarness::with_document(document_with(vec![active, completed, failed, cancelled]));

        assert_eq!(harness.handle.clear_completed().await.unwrap(), 2);
        let snapshot = harness.handle.snapshot();
        assert_eq!(snapshot.jobs.len(), 2);
        assert!(matches!(
            snapshot.jobs[1].state,
            TransferJobState::Failed { .. }
        ));
        assert_eq!(snapshot.summary.failed, 1);
    }

    #[tokio::test]
    async fn settings_require_each_limit_between_one_and_thirty_two() {
        let harness = ActorHarness::new();
        for settings in [
            QueueSettings {
                global_limit: 0,
                per_host_limit: 1,
            },
            QueueSettings {
                global_limit: 33,
                per_host_limit: 1,
            },
            QueueSettings {
                global_limit: 1,
                per_host_limit: 0,
            },
            QueueSettings {
                global_limit: 1,
                per_host_limit: 33,
            },
        ] {
            assert!(harness.handle.update_settings(settings).await.is_err());
        }
        assert_eq!(harness.handle.snapshot().revision, 0);

        let accepted = QueueSettings {
            global_limit: 2,
            per_host_limit: 32,
        };
        harness
            .handle
            .update_settings(accepted.clone())
            .await
            .unwrap();
        assert_eq!(harness.handle.snapshot().settings, accepted);
    }

    #[tokio::test]
    async fn illegal_action_does_not_change_revision_or_emit_events() {
        let harness = ActorHarness::new();
        let id = harness.handle.enqueue(sample_new_job()).await.unwrap();
        harness.handle.pause(id).await.unwrap();
        let before = harness.handle.snapshot();
        let event_count = harness.events.event_count();

        assert!(harness.handle.pause(id).await.is_err());

        assert_eq!(harness.handle.snapshot(), before);
        assert_eq!(harness.events.event_count(), event_count);
    }

    struct MemoryStore {
        document: Mutex<TransferQueueDocument>,
        fail_saves: AtomicBool,
        failed_save_count: AtomicU64,
    }

    impl QueueStore for MemoryStore {
        fn load(&self) -> Result<TransferQueueDocument, String> {
            Ok(self.document.lock().unwrap().clone())
        }

        fn save(&self, document: &TransferQueueDocument) -> Result<(), String> {
            if self.fail_saves.load(Ordering::SeqCst) {
                self.failed_save_count.fetch_add(1, Ordering::SeqCst);
                Err("injected save failure".into())
            } else {
                *self.document.lock().unwrap() = document.clone();
                Ok(())
            }
        }
    }

    struct FixedClock;

    #[async_trait]
    impl QueueClock for FixedClock {
        fn now_ms(&self) -> u64 {
            42
        }

        async fn sleep_until(&self, _unix_ms: u64) {}
    }

    fn actor_without_startup_recovery(
        document: TransferQueueDocument,
        events: Arc<RecordingEventSink>,
    ) -> (QueueActor, Arc<MemoryStore>) {
        let store = Arc::new(MemoryStore {
            document: Mutex::new(document.clone()),
            fail_saves: AtomicBool::new(false),
            failed_save_count: AtomicU64::new(0),
        });
        let snapshot = Arc::new(RwLock::new(TransferQueueSnapshot::from(&document)));
        let (_command_tx, command_rx) = mpsc::unbounded_channel();
        let (runner_event_tx, runner_event_rx) = mpsc::unbounded_channel();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        (
            QueueActor {
                document,
                store: store.clone(),
                event_sink: events,
                clock: Arc::new(FixedClock),
                runner: None,
                command_rx,
                runner_event_tx,
                runner_event_rx,
                internal_tx,
                internal_rx,
                active: HashMap::new(),
                pending_cancel_cleanup: Default::default(),
                pending_artifact_resolutions: Default::default(),
                startup_suspended: false,
                startup_authorized: Default::default(),
                pending_terminal_results: HashMap::new(),
                pending_progress: HashMap::new(),
                persistence_fault: false,
                snapshot,
            },
            store,
        )
    }

    #[tokio::test]
    async fn reorder_and_priority_reject_every_non_queued_state_without_mutation() {
        let states = [
            TransferJobState::Running,
            TransferJobState::Checking,
            TransferJobState::Paused,
            TransferJobState::NeedsAttention {
                reason: AttentionReason::MissingPartial,
            },
        ];
        for (index, state) in states.into_iter().enumerate() {
            let id = Uuid::from_u128(600 + index as u128);
            let document = document_with(vec![stored_job(id, state, 1)]);
            let events = Arc::new(RecordingEventSink::default());
            let (mut actor, store) =
                actor_without_startup_recovery(document.clone(), events.clone());

            assert!(actor.reorder(id, None).await.is_err());
            assert!(
                actor
                    .set_priority(id, TransferPriority::Interactive)
                    .await
                    .is_err()
            );
            assert_eq!(actor.document, document);
            assert_eq!(*store.document.lock().unwrap(), document);
            assert_eq!(events.event_count(), 0);
        }
    }

    #[tokio::test]
    async fn stale_running_and_paused_jobs_without_owned_work_cancel_immediately() {
        let running_id = Uuid::from_u128(701);
        let running_document =
            document_with(vec![stored_job(running_id, TransferJobState::Running, 1)]);
        let running_events = Arc::new(RecordingEventSink::default());
        let (mut actor, running_store) =
            actor_without_startup_recovery(running_document.clone(), running_events.clone());

        assert!(actor.cancel(running_id).await.unwrap());
        assert!(matches!(
            actor.document.jobs[0].state,
            TransferJobState::Cancelled {
                cleanup_error: None
            }
        ));
        assert_eq!(*running_store.document.lock().unwrap(), actor.document);
        assert!(running_events.event_count() > 0);

        let paused_harness = ActorHarness::new();
        let paused_id = paused_harness
            .handle
            .enqueue(new_job(Uuid::from_u128(702), "paused.bin"))
            .await
            .unwrap();
        paused_harness.handle.pause(paused_id).await.unwrap();
        paused_harness.events.clear();

        assert!(paused_harness.handle.cancel(paused_id).await.unwrap());
        assert!(matches!(
            paused_harness.handle.snapshot().jobs[0].state,
            TransferJobState::Cancelled {
                cleanup_error: None
            }
        ));
        assert!(paused_harness.events.event_count() > 0);
    }

    #[tokio::test]
    async fn clean_inactive_states_advertised_by_the_ui_cancel_without_a_runner() {
        let states = [
            TransferJobState::Paused,
            TransferJobState::NeedsConnection {
                message: "reconnect".into(),
            },
            TransferJobState::NeedsAttention {
                reason: AttentionReason::MissingPartial,
            },
        ];
        for (index, state) in states.into_iter().enumerate() {
            let id = Uuid::from_u128(730 + index as u128);
            let harness =
                ActorHarness::with_document(document_with(vec![stored_job(id, state, 1)]));

            assert!(harness.handle.cancel(id).await.unwrap());
            assert_eq!(
                harness.handle.snapshot().jobs[0].state,
                TransferJobState::Cancelled {
                    cleanup_error: None
                }
            );
        }
    }

    #[tokio::test]
    async fn durable_inactive_cancel_starts_cleanup_and_publishes_only_its_result() {
        let id = Uuid::from_u128(740);
        let mut paused = stored_job(id, TransferJobState::Paused, 1);
        paused.durable_checkpoint = 512;
        paused.source_fingerprint = Some(SourceFingerprint {
            size: 1_024,
            modified_token: Some("source-v1".into()),
        });
        paused.artifacts = Some(ManagedArtifacts {
            partial_path: "/tmp/.paused.partial".into(),
            backup_path: "/tmp/.paused.backup".into(),
        });
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document_with(vec![paused]), runner.clone());

        assert!(harness.handle.cancel(id).await.unwrap());
        wait_for_starts(&runner, 1).await;
        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
        assert_eq!(
            harness.handle.snapshot().jobs[0].state,
            TransferJobState::Paused
        );

        release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
        wait_for_job(&harness.handle, id, |job| {
            matches!(
                job.state,
                TransferJobState::Cancelled {
                    cleanup_error: None
                }
            )
        })
        .await;
    }

    #[tokio::test]
    async fn skip_waits_for_owned_commit_cleanup_before_persisting_completion() {
        let id = Uuid::from_u128(747);
        let mut job = durable_cancel_job(id, "host-a", 1);
        job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::CommitRecovery {
                message: "owned backup and partial require reconciliation".into(),
            },
        };
        job.commit_phase = CommitPhase::BackupMoved;
        job.commit_backup_expected = Some(true);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document_with(vec![job]), runner.clone());
        let original = harness.handle.snapshot().jobs[0].clone();

        harness
            .handle
            .resolve(id, ConflictResolution::Skip)
            .await
            .unwrap();
        wait_for_starts(&runner, 1).await;

        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
        assert_eq!(harness.handle.snapshot().jobs[0], original);
        assert_eq!(
            harness.store.load().unwrap().into_document().jobs[0],
            original,
            "the durable document must keep ownership until cleanup acknowledges"
        );
        assert!(
            harness
                .handle
                .resolve(
                    id,
                    ConflictResolution::Rename {
                        destination: "/srv/racing.bin".into(),
                    },
                )
                .await
                .unwrap_err()
                .contains("cleanup owns")
        );

        release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
        let skipped = wait_for_job(&harness.handle, id, |job| {
            matches!(
                job.state,
                TransferJobState::Completed {
                    result: CompletionResult::Skipped
                }
            )
        })
        .await;
        assert_eq!(skipped.artifacts, None);
        assert_eq!(skipped.source_fingerprint, None);
        assert_eq!(skipped.durable_checkpoint, 0);
        assert_eq!(skipped.commit_phase, CommitPhase::None);
        assert_eq!(
            harness.store.load().unwrap().into_document().jobs[0],
            skipped
        );
    }

    #[tokio::test]
    async fn rename_cleanup_failure_preserves_old_destination_and_exact_artifact_identity() {
        let id = Uuid::from_u128(748);
        let mut job = durable_cancel_job(id, "host-a", 1);
        job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: true,
            },
        };
        let original_destination = job.remote_path.clone();
        let original_artifacts = job.artifacts.clone();
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document_with(vec![job]), runner.clone());

        harness
            .handle
            .resolve(
                id,
                ConflictResolution::Rename {
                    destination: "/srv/renamed.bin".into(),
                },
            )
            .await
            .unwrap();
        wait_for_starts(&runner, 1).await;
        release
            .send(RunnerResult::NeedsAttention(AttentionReason::Cleanup {
                message: "/srv/.owned.termlab-backup-id could not be removed".into(),
            }))
            .unwrap();

        let attention = wait_for_job(&harness.handle, id, |job| {
            matches!(
                job.state,
                TransferJobState::NeedsAttention {
                    reason: AttentionReason::Cleanup { .. }
                }
            )
        })
        .await;
        assert_eq!(attention.remote_path, original_destination);
        assert_eq!(attention.artifacts, original_artifacts);
        let TransferJobState::NeedsAttention {
            reason: AttentionReason::Cleanup { message },
        } = attention.state
        else {
            unreachable!()
        };
        assert!(message.contains("/srv/.owned.termlab-backup-id"));
    }

    #[tokio::test]
    async fn inactive_cancel_can_quarantine_an_unsafe_cleanup_result() {
        let id = Uuid::from_u128(746);
        let job = durable_cancel_job(id, "host-a", 1);
        let runner = Arc::new(GatedRunner::default());
        let release = runner.gate(id);
        let harness = ActorHarness::with_runner(document_with(vec![job]), runner.clone());

        assert!(harness.handle.cancel(id).await.unwrap());
        wait_for_starts(&runner, 1).await;
        release
            .send(RunnerResult::NeedsAttention(
                AttentionReason::CommitRecovery {
                    message: "mismatched managed paths were preserved".into(),
                },
            ))
            .unwrap();

        let quarantined = wait_for_job(&harness.handle, id, |job| {
            matches!(
                job.state,
                TransferJobState::NeedsAttention {
                    reason: AttentionReason::CommitRecovery { .. }
                }
            )
        })
        .await;
        let TransferJobState::NeedsAttention {
            reason: AttentionReason::CommitRecovery { message },
        } = quarantined.state
        else {
            unreachable!()
        };
        assert!(message.contains("preserved"));
    }

    #[tokio::test]
    async fn pending_cancel_cleanup_obeys_global_capacity_and_releases_it() {
        let ordinary = Uuid::from_u128(741);
        let cleanup = Uuid::from_u128(742);
        let mut document = document_with(vec![
            on_host_with_destination(
                stored_job(ordinary, TransferJobState::Queued, 1),
                "host-a",
                "host-a:/srv/ordinary.bin",
            ),
            durable_cancel_job(cleanup, "host-b", 2),
        ]);
        document.queue_paused = false;
        document.settings = QueueSettings {
            global_limit: 1,
            per_host_limit: 1,
        };
        let runner = Arc::new(GatedRunner::default());
        let ordinary_release = runner.gate(ordinary);
        let cleanup_release = runner.gate(cleanup);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        assert_eq!(runner.starts(), [ordinary]);
        assert_eq!(runner.control_state(ordinary), RunnerControlState::Run);
        assert!(harness.handle.cancel(cleanup).await.unwrap());
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(runner.starts(), [ordinary]);

        ordinary_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [ordinary, cleanup]);
        assert_eq!(runner.control_state(cleanup), RunnerControlState::Cancel);
        cleanup_release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn pending_cancel_cleanup_obeys_per_host_capacity() {
        let first_host_a = Uuid::from_u128(743);
        let second_host_a = Uuid::from_u128(744);
        let host_b = Uuid::from_u128(745);
        let mut document = document_with(vec![
            on_host_with_destination(
                stored_job(first_host_a, TransferJobState::Queued, 1),
                "host-a",
                "host-a:/srv/ordinary.bin",
            ),
            {
                let mut job = durable_cancel_job(second_host_a, "host-a", 2);
                job.state = TransferJobState::NeedsAttention {
                    reason: AttentionReason::MissingPartial,
                };
                job
            },
            {
                let mut job = durable_cancel_job(host_b, "host-b", 3);
                job.state = TransferJobState::NeedsAttention {
                    reason: AttentionReason::MissingPartial,
                };
                job
            },
        ]);
        document.queue_paused = false;
        document.settings = QueueSettings {
            global_limit: 2,
            per_host_limit: 1,
        };
        let runner = Arc::new(GatedRunner::default());
        let first_release = runner.gate(first_host_a);
        let second_release = runner.gate(second_host_a);
        let host_b_release = runner.gate(host_b);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        assert_eq!(runner.starts(), [first_host_a]);
        assert!(harness.handle.cancel(second_host_a).await.unwrap());
        assert!(harness.handle.cancel(host_b).await.unwrap());
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [first_host_a, host_b]);

        first_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_starts(&runner, 3).await;
        assert_eq!(runner.starts(), [first_host_a, host_b, second_host_a]);
        assert_eq!(
            runner.control_state(second_host_a),
            RunnerControlState::Cancel
        );
        host_b_release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
        second_release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn cancellation_ownership_rejects_pending_and_active_lifecycle_races() {
        let blocker = Uuid::from_u128(750);
        let attention = Uuid::from_u128(751);
        let paused = Uuid::from_u128(752);
        let disconnected = Uuid::from_u128(753);
        let queued = Uuid::from_u128(754);
        let mut attention_job = durable_cancel_job(attention, "host-attention", 2);
        attention_job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::MissingPartial,
        };
        let paused_job = durable_cancel_job(paused, "host-paused", 3);
        let mut disconnected_job = durable_cancel_job(disconnected, "host-disconnected", 4);
        disconnected_job.state = TransferJobState::NeedsConnection {
            message: "reconnect".into(),
        };
        let mut queued_job = durable_cancel_job(queued, "host-queued", 5);
        queued_job.state = TransferJobState::Queued;
        let mut document = document_with(vec![
            on_host_with_destination(
                stored_job(blocker, TransferJobState::Queued, 1),
                "host-blocker",
                "host-blocker:/srv/blocker.bin",
            ),
            attention_job,
            paused_job,
            disconnected_job,
            queued_job,
        ]);
        document.queue_paused = false;
        document.settings = QueueSettings {
            global_limit: 1,
            per_host_limit: 1,
        };
        let runner = Arc::new(GatedRunner::default());
        let blocker_release = runner.gate(blocker);
        let attention_release = runner.gate(attention);
        let paused_release = runner.gate(paused);
        let disconnected_release = runner.gate(disconnected);
        let queued_release = runner.gate(queued);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        assert_eq!(runner.starts(), [blocker]);
        for id in [attention, paused, disconnected, queued] {
            assert!(harness.handle.cancel(id).await.unwrap());
        }

        let resolve_error = harness
            .handle
            .resolve(attention, ConflictResolution::Skip)
            .await
            .unwrap_err();
        let resume_error = harness.handle.resume(paused).await.unwrap_err();
        let retry_error = harness.handle.retry(disconnected).await.unwrap_err();
        let reorder_error = harness.handle.reorder(queued, None).await.unwrap_err();
        let priority_error = harness
            .handle
            .set_priority(queued, TransferPriority::Interactive)
            .await
            .unwrap_err();
        for error in [
            resolve_error,
            resume_error,
            retry_error,
            reorder_error,
            priority_error,
        ] {
            assert!(error.contains("cancellation cleanup owns"), "{error}");
        }

        blocker_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [blocker, attention]);
        assert!(
            harness
                .handle
                .resolve(attention, ConflictResolution::Skip)
                .await
                .unwrap_err()
                .contains("cancellation cleanup owns")
        );
        harness.handle.pause(attention).await.unwrap();
        harness.handle.pause_all().await.unwrap();
        assert_eq!(runner.control_state(attention), RunnerControlState::Cancel);

        for (count, id, release) in [
            (3, attention, attention_release),
            (4, paused, paused_release),
            (5, disconnected, disconnected_release),
            (6, queued, queued_release),
        ] {
            release
                .send(RunnerResult::Cancelled {
                    cleanup_error: None,
                })
                .unwrap();
            wait_for_job(&harness.handle, id, |job| {
                matches!(job.state, TransferJobState::Cancelled { .. })
            })
            .await;
            if count < 6 {
                wait_for_starts(&runner, count).await;
            }
        }
        assert!(harness.handle.snapshot().jobs.iter().all(|job| {
            job.id == blocker || matches!(job.state, TransferJobState::Cancelled { .. })
        }));
    }

    #[tokio::test]
    async fn reorder_rejects_when_an_indirectly_affected_job_is_cleanup_owned() {
        let blocker = Uuid::from_u128(757);
        let cleanup = Uuid::from_u128(758);
        let ordinary = Uuid::from_u128(759);
        let mut cleanup_job = durable_cancel_job(cleanup, "host-cleanup", 2);
        cleanup_job.state = TransferJobState::Queued;
        let mut document = document_with(vec![
            on_host_with_destination(
                stored_job(blocker, TransferJobState::Queued, 1),
                "host-blocker",
                "host-blocker:/srv/blocker.bin",
            ),
            cleanup_job,
            on_host_with_destination(
                stored_job(ordinary, TransferJobState::Queued, 3),
                "host-ordinary",
                "host-ordinary:/srv/ordinary.bin",
            ),
        ]);
        document.queue_paused = false;
        document.settings = QueueSettings {
            global_limit: 1,
            per_host_limit: 1,
        };
        let runner = Arc::new(GatedRunner::default());
        let blocker_release = runner.gate(blocker);
        let cleanup_release = runner.gate(cleanup);
        let harness = ActorHarness::with_runner(document, runner.clone());

        harness.handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        assert_eq!(runner.starts(), [blocker]);
        assert!(harness.handle.cancel(cleanup).await.unwrap());
        let before = harness.handle.snapshot();
        let cleanup_before = before
            .jobs
            .iter()
            .find(|job| job.id == cleanup)
            .unwrap()
            .clone();
        let ordinary_before = before
            .jobs
            .iter()
            .find(|job| job.id == ordinary)
            .unwrap()
            .clone();
        harness.events.clear();

        let error = harness
            .handle
            .reorder(ordinary, Some(cleanup))
            .await
            .unwrap_err();

        assert!(error.contains("cancellation cleanup owns"), "{error}");
        let after = harness.handle.snapshot();
        assert_eq!(
            after.jobs.iter().find(|job| job.id == cleanup).unwrap(),
            &cleanup_before
        );
        assert_eq!(
            after.jobs.iter().find(|job| job.id == ordinary).unwrap(),
            &ordinary_before
        );
        assert_eq!(harness.events.event_count(), 0);

        blocker_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [blocker, cleanup]);
        assert_eq!(runner.control_state(cleanup), RunnerControlState::Cancel);
        cleanup_release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn rejected_cleanup_result_quarantines_job_and_releases_capacity() {
        let rejected = Uuid::from_u128(755);
        let waiting = Uuid::from_u128(756);
        let mut rejected_job = durable_cancel_job(rejected, "host-a", 1);
        rejected_job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::MissingPartial,
        };
        let mut waiting_job = durable_cancel_job(waiting, "host-b", 2);
        waiting_job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::MissingPartial,
        };
        let mut document = document_with(vec![rejected_job, waiting_job]);
        document.settings = QueueSettings {
            global_limit: 1,
            per_host_limit: 1,
        };
        let runner = Arc::new(GatedRunner::default());
        let rejected_release = runner.gate(rejected);
        let waiting_release = runner.gate(waiting);
        let harness = ActorHarness::with_runner(document, runner.clone());

        assert!(harness.handle.cancel(rejected).await.unwrap());
        wait_for_starts(&runner, 1).await;
        assert!(harness.handle.cancel(waiting).await.unwrap());
        rejected_release
            .send(RunnerResult::Paused {
                durable_checkpoint: 999,
            })
            .unwrap();

        let quarantined = wait_for_job(&harness.handle, rejected, |job| {
            matches!(
                job.state,
                TransferJobState::NeedsAttention {
                    reason: AttentionReason::Cleanup { .. }
                }
            )
        })
        .await;
        let TransferJobState::NeedsAttention {
            reason: AttentionReason::Cleanup { message },
        } = quarantined.state
        else {
            unreachable!()
        };
        assert!(message.contains("pause"), "{message}");
        assert_eq!(
            quarantined.durable_checkpoint, 512,
            "a rejected result cannot partially mutate the quarantined job"
        );
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [rejected, waiting]);
        assert_eq!(runner.control_state(waiting), RunnerControlState::Cancel);
        waiting_release
            .send(RunnerResult::Cancelled {
                cleanup_error: None,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn queued_jobs_with_cleanup_work_cannot_claim_cleanup_success() {
        for (index, decorate) in ["artifacts", "checkpoint", "commit_phase"]
            .into_iter()
            .enumerate()
        {
            let id = Uuid::from_u128(710 + index as u128);
            let mut job = stored_job(id, TransferJobState::Queued, 1);
            match decorate {
                "artifacts" => {
                    job.artifacts = Some(ManagedArtifacts {
                        partial_path: "/tmp/partial".into(),
                        backup_path: "/tmp/backup".into(),
                    });
                }
                "checkpoint" => job.durable_checkpoint = 512,
                "commit_phase" => job.commit_phase = CommitPhase::Prepared,
                _ => unreachable!(),
            }
            let harness = ActorHarness::with_document(document_with(vec![job]));
            let before = harness.handle.snapshot();
            harness.events.clear();

            assert!(harness.handle.cancel(id).await.is_err());
            assert_eq!(harness.handle.snapshot(), before);
            assert_eq!(harness.events.event_count(), 0);
        }
    }

    #[tokio::test]
    async fn save_failure_keeps_snapshot_revision_and_events_unchanged() {
        let store = Arc::new(MemoryStore {
            document: Mutex::new(TransferQueueDocument::default()),
            fail_saves: AtomicBool::new(false),
            failed_save_count: AtomicU64::new(0),
        });
        let events = Arc::new(RecordingEventSink::default());
        let handle =
            QueueActor::spawn_with_services(store.clone(), events.clone(), Arc::new(FixedClock))
                .unwrap();
        store.fail_saves.store(true, Ordering::SeqCst);

        let error = handle.enqueue(sample_new_job()).await.unwrap_err();

        assert!(error.contains("injected save failure"));
        assert_eq!(handle.snapshot().revision, 0);
        assert!(handle.snapshot().jobs.is_empty());
        assert_eq!(events.event_count(), 0);
    }

    #[tokio::test]
    async fn terminal_save_failure_retains_result_blocks_dispatch_and_recovers_once() {
        let first = Uuid::from_u128(8_801);
        let second = Uuid::from_u128(8_802);
        let document = document_with(vec![stored_job(first, TransferJobState::Queued, 1)]);
        let store = Arc::new(MemoryStore {
            document: Mutex::new(document),
            fail_saves: AtomicBool::new(false),
            failed_save_count: AtomicU64::new(0),
        });
        let events = Arc::new(RecordingEventSink::default());
        let runner = Arc::new(GatedRunner::default());
        let first_release = runner.gate(first);
        let _second_release = runner.gate(second);
        let clock = Arc::new(FakeClock::new(30_000));
        let handle = QueueActor::spawn_with_runner_services(
            store.clone(),
            events.clone(),
            clock.clone(),
            runner.clone(),
        )
        .unwrap();
        handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;

        store.fail_saves.store(true, Ordering::SeqCst);
        first_release
            .send(RunnerResult::Completed(CompletionResult::Transferred))
            .unwrap();
        for _ in 0..100 {
            if store.failed_save_count.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(store.failed_save_count.load(Ordering::SeqCst), 1);
        assert!(matches!(
            handle.snapshot().jobs[0].state,
            TransferJobState::Connecting
        ));

        store.fail_saves.store(false, Ordering::SeqCst);
        handle
            .enqueue(new_job(second, "after-recovery.bin"))
            .await
            .unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            runner.starts(),
            [first],
            "a pending terminal result suspends all new dispatch"
        );

        clock.advance_to(30_250);
        let completed = wait_for_job(&handle, first, |job| {
            matches!(
                job.state,
                TransferJobState::Completed {
                    result: CompletionResult::Transferred
                }
            )
        })
        .await;
        assert_eq!(completed.commit_phase, CommitPhase::Complete);
        wait_for_starts(&runner, 2).await;
        assert_eq!(runner.starts(), [first, second]);
        let terminal_updates = events
            .deltas
            .lock()
            .unwrap()
            .iter()
            .flat_map(|delta| &delta.upserts)
            .filter(|job| {
                job.id == first && matches!(job.state, TransferJobState::Completed { .. })
            })
            .count();
        assert_eq!(
            terminal_updates, 1,
            "the retained result commits exactly once"
        );
    }

    #[tokio::test]
    async fn result_persistence_failure_releases_lease_before_retrying_durably() {
        let id = Uuid::from_u128(8_820);
        let mut job = durable_cancel_job(id, "host-a", 1);
        job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::MissingPartial,
        };
        let events = Arc::new(RecordingEventSink::default());
        let (mut actor, store) = actor_without_startup_recovery(document_with(vec![job]), events);
        let lease_id = Uuid::from_u128(8_821);
        let (control_tx, _control_rx) = watch::channel(RunnerControlState::Cancel);
        actor.active.insert(
            id,
            super::ActiveTask {
                lease_id,
                lease: crate::remote::transfer_queue::scheduler::ActiveLease {
                    job_id: id,
                    host_key: "host-a".into(),
                    destination_key: "host-a:/srv/result.bin".into(),
                },
                ownership: super::JobOwnership::CancellationCleanup,
                control_tx,
                commit_critical: false,
                deferred_control: None,
                resume_after_pause: false,
            },
        );
        actor.pending_progress.insert(
            id,
            super::PendingProgress {
                lease_id,
                bytes: 128,
                speed_bytes_per_second: Some(64),
                eta_seconds: Some(2),
                deadline_ms: 300,
            },
        );
        store.fail_saves.store(true, Ordering::SeqCst);

        let error = actor
            .runner_finished(
                id,
                lease_id,
                RunnerResult::Cancelled {
                    cleanup_error: None,
                },
            )
            .await
            .unwrap_err();

        assert!(error.contains("injected save failure"));
        assert!(!actor.active.contains_key(&id), "runner lease was stranded");
        assert!(!actor.pending_progress.contains_key(&id));
        assert!(actor.persistence_fault);
        assert!(
            actor.cancel(id).await.unwrap(),
            "repeated cancellation remains owned while persistence retries"
        );
        assert!(actor.pending_cancel_cleanup.is_empty());

        store.fail_saves.store(false, Ordering::SeqCst);
        actor
            .persist_pending_runner_result(id, lease_id)
            .await
            .unwrap();
        assert!(matches!(
            actor.document.jobs[0].state,
            TransferJobState::Cancelled { .. }
        ));
        assert!(!actor.persistence_fault);
    }

    #[tokio::test]
    async fn prepared_persistence_failure_rejects_ack_and_restores_pending_cancel() {
        let id = Uuid::from_u128(8_850);
        let document = document_with(vec![stored_job(id, TransferJobState::Queued, 1)]);
        let store = Arc::new(MemoryStore {
            document: Mutex::new(document),
            fail_saves: AtomicBool::new(false),
            failed_save_count: AtomicU64::new(0),
        });
        let events = Arc::new(RecordingEventSink::default());
        let runner = Arc::new(GatedRunner::default());
        let _release = runner.gate(id);
        let handle = QueueActor::spawn_with_runner_services(
            store.clone(),
            events,
            Arc::new(FixedClock),
            runner.clone(),
        )
        .unwrap();
        handle.resume_all().await.unwrap();
        wait_for_starts(&runner, 1).await;
        let reporter = runner.reporter(id);
        assert!(handle.cancel(id).await.unwrap());

        store.fail_saves.store(true, Ordering::SeqCst);
        let error = reporter
            .commit_phase(CommitPhase::Prepared)
            .await
            .unwrap_err();

        assert!(error.contains("injected save failure"));
        assert_eq!(runner.control_state(id), RunnerControlState::Cancel);
        assert_eq!(handle.snapshot().jobs[0].commit_phase, CommitPhase::None);
        assert_eq!(
            store.document.lock().unwrap().jobs[0].commit_phase,
            CommitPhase::None
        );
    }

    #[tokio::test]
    async fn startup_recovers_and_persists_suspension_without_starting_work() {
        let id = Uuid::from_u128(501);
        let mut document = document_with(vec![stored_job(id, TransferJobState::Running, 1)]);
        document.queue_paused = false;
        let store = Arc::new(MemoryStore {
            document: Mutex::new(document),
            fail_saves: AtomicBool::new(false),
            failed_save_count: AtomicU64::new(0),
        });
        let events = Arc::new(RecordingEventSink::default());

        let handle =
            QueueActor::spawn_with_services(store.clone(), events.clone(), Arc::new(FixedClock))
                .unwrap();

        let snapshot = handle.snapshot();
        assert!(snapshot.queue_paused);
        assert_eq!(snapshot.jobs[0].state, TransferJobState::Paused);
        assert_eq!(store.document.lock().unwrap().revision, snapshot.revision);
        assert_eq!(events.event_count(), 0);
    }

    #[test]
    fn bootstrap_exposes_quarantine_error_and_suspension_before_actor_start() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("transfers.json");
        std::fs::write(&path, b"not json").unwrap();

        let (bootstrap, handle) = QueueActor::bootstrap(TransferStore::new(path.clone())).unwrap();

        let snapshot = handle.snapshot();
        assert!(snapshot.queue_paused);
        assert!(snapshot.jobs.is_empty());
        assert!(
            snapshot
                .recovery_error
                .as_deref()
                .is_some_and(|message| message.contains("quarantined"))
        );
        assert!(
            path.exists(),
            "the recovered snapshot is persisted for late subscribers"
        );
        drop(bootstrap);
    }
}
