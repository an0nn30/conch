use std::{
    collections::HashSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::{
    events::{QueueEventPayload, QueueSummaryPayload, TransferEventSink, legacy_progress_for},
    model::{
        CommitPhase, ConflictResolution, NewTransferJob, QueueSettings, TRANSFER_HISTORY_LIMIT,
        TransferEndpoint, TransferJob, TransferJobState, TransferPriority, TransferQueueDocument,
        TransferQueueSnapshot, build_destination_key,
    },
    reducer::{JobEvent, reduce_job},
    store::{TransferStore, recover_for_startup},
};

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

pub trait QueueClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

struct SystemQueueClock;

impl QueueClock for SystemQueueClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
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

pub struct QueueActor {
    document: TransferQueueDocument,
    store: Arc<dyn QueueStore>,
    event_sink: Arc<dyn TransferEventSink>,
    clock: Arc<dyn QueueClock>,
    command_rx: mpsc::UnboundedReceiver<QueueCommand>,
    snapshot: Arc<RwLock<TransferQueueSnapshot>>,
}

impl QueueActor {
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
        let mut document = store.load()?;
        recover_for_startup(&mut document);
        validate_document(&document)?;
        store.save(&document)?;
        let snapshot = Arc::new(RwLock::new(TransferQueueSnapshot::from(&document)));
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let actor = Self {
            document,
            store,
            event_sink,
            clock,
            command_rx,
            snapshot: snapshot.clone(),
        };
        tokio::spawn(actor.run());
        Ok(TransferQueueHandle {
            command_tx,
            snapshot,
        })
    }

    async fn run(mut self) {
        while let Some(command) = self.command_rx.recv().await {
            match command {
                QueueCommand::Enqueue { request, reply } => {
                    let result = self.enqueue(request).await;
                    let _ = reply.send(result);
                }
                QueueCommand::Pause { id, reply } => {
                    let result = self.apply_job_event(id, JobEvent::Pause).await;
                    let _ = reply.send(result);
                }
                QueueCommand::Resume { id, reply } => {
                    let result = self.apply_job_event(id, JobEvent::Resume).await;
                    let _ = reply.send(result);
                }
                QueueCommand::PauseAll { reply } => {
                    let result = self.set_queue_paused(true).await;
                    let _ = reply.send(result);
                }
                QueueCommand::ResumeAll { reply } => {
                    let result = self.set_queue_paused(false).await;
                    let _ = reply.send(result);
                }
                QueueCommand::Cancel { id, reply } => {
                    let result = self.cancel(id).await;
                    let _ = reply.send(result);
                }
                QueueCommand::Retry { id, reply } => {
                    let result = self.apply_job_event(id, JobEvent::ManualRetry).await;
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
            conflict_policy,
        } = request;
        let host_key = host_key(&endpoint);
        let destination_key =
            build_destination_key(&host_key, &direction, &local_path, &remote_path);
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
            priority: TransferPriority::Normal,
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
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            started_at_ms: None,
            finished_at_ms: None,
        };

        let mut next = self.document.clone();
        next.jobs.push(job);
        self.commit(next).await?;
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

    async fn set_queue_paused(&mut self, paused: bool) -> Result<(), String> {
        if self.document.queue_paused == paused {
            return Ok(());
        }
        let mut next = self.document.clone();
        next.queue_paused = paused;
        self.commit(next).await
    }

    async fn cancel(&mut self, id: Uuid) -> Result<bool, String> {
        let Some(job) = self.document.jobs.iter().find(|job| job.id == id) else {
            return Ok(false);
        };
        if !matches!(job.state, TransferJobState::Queued) {
            return Err(format!(
                "transfer job {id} cannot be cancelled until runner cleanup is acknowledged"
            ));
        }
        if job.artifacts.is_some()
            || job.durable_checkpoint != 0
            || job.commit_phase != CommitPhase::None
        {
            return Err(format!(
                "transfer job {id} owns durable work that requires runner cleanup"
            ));
        }
        self.apply_job_event(id, JobEvent::Cancel(None)).await?;
        Ok(true)
    }

    async fn resolve(&mut self, id: Uuid, resolution: ConflictResolution) -> Result<(), String> {
        if matches!(
            &resolution,
            ConflictResolution::Rename { destination } if destination.trim().is_empty()
        ) {
            return Err("rename destination must not be empty".into());
        }
        self.apply_job_event(id, JobEvent::Resolve(resolution))
            .await
    }

    async fn reorder(&mut self, id: Uuid, before: Option<Uuid>) -> Result<(), String> {
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

        let now_ms = self.clock.now_ms();
        let mut next = self.document.clone();
        let mut changed = Vec::new();
        for ((job_id, _, _), order) in ordered.iter().zip(queue_order_slots) {
            let job = next
                .jobs
                .iter_mut()
                .find(|job| job.id == *job_id)
                .expect("ordered jobs came from this document");
            if job.queue_order != order {
                job.queue_order = order;
                job.updated_at_ms = now_ms;
                changed.push(*job_id);
            }
        }
        if changed.is_empty() {
            return Ok(());
        }
        self.commit(next).await
    }

    async fn set_priority(&mut self, id: Uuid, priority: TransferPriority) -> Result<(), String> {
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
        validate_settings(&settings)?;
        if self.document.settings == settings {
            return Ok(());
        }
        let mut next = self.document.clone();
        next.settings = settings;
        self.commit(next).await
    }

    async fn commit(&mut self, mut next: TransferQueueDocument) -> Result<(), String> {
        compact_history(&mut next);
        validate_document(&next)?;
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
    Ok(())
}

fn host_key(endpoint: &TransferEndpoint) -> String {
    match endpoint {
        TransferEndpoint::Configured {
            server_entry_id, ..
        } => format!("configured:{server_entry_id}"),
        TransferEndpoint::AdHoc {
            host, port, user, ..
        } => format!("adhoc:{user}@{host}:{port}"),
    }
}

fn validate_settings(settings: &QueueSettings) -> Result<(), String> {
    if !(1..=32).contains(&settings.global_limit) {
        return Err("global transfer limit must be between 1 and 32".into());
    }
    if !(1..=32).contains(&settings.per_host_limit) {
        return Err("per-host transfer limit must be between 1 and 32".into());
    }
    Ok(())
}

fn validate_document(document: &TransferQueueDocument) -> Result<(), String> {
    validate_settings(&document.settings)?;
    let mut ids = HashSet::new();
    let mut active_orders = HashSet::new();
    for job in &document.jobs {
        if !ids.insert(job.id) {
            return Err(format!("transfer queue contains duplicate job {}", job.id));
        }
        if !job.state.is_terminal()
            && (job.queue_order == 0 || !active_orders.insert(job.queue_order))
        {
            return Err("active transfer queue orders must be unique and non-zero".into());
        }
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
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
    };

    use async_trait::async_trait;
    use parking_lot::RwLock;
    use tokio::sync::mpsc;
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
        store::TransferStore,
    };

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
            created_at_ms: 10,
            updated_at_ms: 10,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }

    fn document_with(jobs: Vec<TransferJob>) -> TransferQueueDocument {
        TransferQueueDocument {
            jobs,
            ..TransferQueueDocument::default()
        }
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
    }

    impl QueueStore for MemoryStore {
        fn load(&self) -> Result<TransferQueueDocument, String> {
            Ok(self.document.lock().unwrap().clone())
        }

        fn save(&self, document: &TransferQueueDocument) -> Result<(), String> {
            if self.fail_saves.load(Ordering::SeqCst) {
                Err("injected save failure".into())
            } else {
                *self.document.lock().unwrap() = document.clone();
                Ok(())
            }
        }
    }

    struct FixedClock;

    impl QueueClock for FixedClock {
        fn now_ms(&self) -> u64 {
            42
        }
    }

    fn actor_without_startup_recovery(
        document: TransferQueueDocument,
        events: Arc<RecordingEventSink>,
    ) -> (QueueActor, Arc<MemoryStore>) {
        let store = Arc::new(MemoryStore {
            document: Mutex::new(document.clone()),
            fail_saves: AtomicBool::new(false),
        });
        let snapshot = Arc::new(RwLock::new(TransferQueueSnapshot::from(&document)));
        let (_command_tx, command_rx) = mpsc::unbounded_channel();
        (
            QueueActor {
                document,
                store: store.clone(),
                event_sink: events,
                clock: Arc::new(FixedClock),
                command_rx,
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
    async fn running_and_paused_cancellation_wait_for_runner_cleanup_acknowledgement() {
        let running_id = Uuid::from_u128(701);
        let running_document =
            document_with(vec![stored_job(running_id, TransferJobState::Running, 1)]);
        let running_events = Arc::new(RecordingEventSink::default());
        let (mut actor, running_store) =
            actor_without_startup_recovery(running_document.clone(), running_events.clone());

        assert!(actor.cancel(running_id).await.is_err());
        assert_eq!(actor.document, running_document);
        assert_eq!(*running_store.document.lock().unwrap(), running_document);
        assert_eq!(running_events.event_count(), 0);

        let paused_harness = ActorHarness::new();
        let paused_id = paused_harness
            .handle
            .enqueue(new_job(Uuid::from_u128(702), "paused.bin"))
            .await
            .unwrap();
        paused_harness.handle.pause(paused_id).await.unwrap();
        let before = paused_harness.handle.snapshot();
        paused_harness.events.clear();

        assert!(paused_harness.handle.cancel(paused_id).await.is_err());
        assert_eq!(paused_harness.handle.snapshot(), before);
        assert_eq!(paused_harness.events.event_count(), 0);
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
    async fn startup_recovers_and_persists_suspension_without_starting_work() {
        let id = Uuid::from_u128(501);
        let mut document = document_with(vec![stored_job(id, TransferJobState::Running, 1)]);
        document.queue_paused = false;
        let store = Arc::new(MemoryStore {
            document: Mutex::new(document),
            fail_saves: AtomicBool::new(false),
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
}
