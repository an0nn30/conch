use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use termlab_remote::transfer::SourceFingerprint;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use super::{
    events::RunnerEvent,
    model::{AttentionReason, CommitPhase, CompletionResult, ManagedArtifacts, TransferJob},
    scheduler::FailureClass,
};

#[async_trait]
pub trait TransferJobRunner: Send + Sync {
    async fn run(
        &self,
        job: TransferJob,
        control: RunnerControl,
        reporter: RunnerReporter,
    ) -> RunnerResult;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerControlState {
    Run,
    Pause,
    Cancel,
}

#[derive(Clone)]
pub struct RunnerControl {
    state: watch::Receiver<RunnerControlState>,
}

impl RunnerControl {
    pub(crate) fn new(state: watch::Receiver<RunnerControlState>) -> Self {
        Self { state }
    }

    pub fn state(&self) -> RunnerControlState {
        *self.state.borrow()
    }

    pub async fn changed(&mut self) -> Result<RunnerControlState, String> {
        self.state
            .changed()
            .await
            .map_err(|_| "transfer runner control channel closed".to_string())?;
        Ok(self.state())
    }
}

#[derive(Debug, Clone)]
pub enum RunnerResult {
    Completed(CompletionResult),
    Paused {
        durable_checkpoint: u64,
    },
    Cancelled {
        cleanup_error: Option<String>,
    },
    NeedsConnection(String),
    NeedsAttention(AttentionReason),
    Failed {
        class: FailureClass,
        message: String,
    },
}

#[derive(Clone)]
pub struct RunnerReporter {
    job_id: Uuid,
    lease_id: Uuid,
    event_tx: mpsc::UnboundedSender<RunnerEvent>,
    progress_slot: Arc<ProgressSlot>,
}

impl RunnerReporter {
    pub(crate) fn new(
        job_id: Uuid,
        lease_id: Uuid,
        event_tx: mpsc::UnboundedSender<RunnerEvent>,
    ) -> Self {
        Self {
            job_id,
            lease_id,
            event_tx,
            progress_slot: Arc::new(ProgressSlot::default()),
        }
    }

    pub async fn fingerprinted(
        &self,
        fingerprint: SourceFingerprint,
        total_bytes: u64,
        artifacts: ManagedArtifacts,
    ) -> Result<(), String> {
        let (ack, response) = oneshot::channel();
        self.send(RunnerEvent::Fingerprinted {
            job_id: self.job_id,
            lease_id: self.lease_id,
            fingerprint,
            total_bytes,
            artifacts,
            ack,
        })?;
        await_ack(response).await
    }

    pub async fn durable_checkpoint(&self, bytes: u64) -> Result<(), String> {
        let (ack, response) = oneshot::channel();
        self.send(RunnerEvent::DurableCheckpoint {
            job_id: self.job_id,
            lease_id: self.lease_id,
            bytes,
            ack,
        })?;
        await_ack(response).await
    }

    pub async fn commit_phase(&self, phase: CommitPhase) -> Result<(), String> {
        let (ack, response) = oneshot::channel();
        self.send(RunnerEvent::CommitPhase {
            job_id: self.job_id,
            lease_id: self.lease_id,
            phase,
            ack,
        })?;
        await_ack(response).await
    }

    pub fn progress(&self, bytes: u64, speed_bytes_per_second: u64, eta_seconds: Option<u64>) {
        let progress = RunnerProgress {
            bytes,
            speed_bytes_per_second,
            eta_seconds,
        };
        if self.progress_slot.publish(progress)
            && self
                .send(RunnerEvent::ProgressReady {
                    job_id: self.job_id,
                    lease_id: self.lease_id,
                    slot: self.progress_slot.clone(),
                })
                .is_err()
        {
            self.progress_slot.release_failed_wake();
        }
    }

    fn send(&self, event: RunnerEvent) -> Result<(), String> {
        self.event_tx
            .send(event)
            .map_err(|_| "transfer queue actor is unavailable".to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunnerProgress {
    pub(crate) bytes: u64,
    pub(crate) speed_bytes_per_second: u64,
    pub(crate) eta_seconds: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct ProgressSlot {
    state: Mutex<ProgressSlotState>,
}

#[derive(Debug, Default)]
struct ProgressSlotState {
    latest: Option<RunnerProgress>,
    wake_queued: bool,
}

impl ProgressSlot {
    fn publish(&self, progress: RunnerProgress) -> bool {
        let mut state = self.state.lock().expect("progress slot lock poisoned");
        state.latest = Some(progress);
        if state.wake_queued {
            false
        } else {
            state.wake_queued = true;
            true
        }
    }

    pub(crate) fn take_latest_and_release_wake(&self) -> Option<RunnerProgress> {
        let mut state = self.state.lock().expect("progress slot lock poisoned");
        let latest = state.latest.take();
        state.wake_queued = false;
        latest
    }

    fn release_failed_wake(&self) {
        self.state
            .lock()
            .expect("progress slot lock poisoned")
            .wake_queued = false;
    }
}

async fn await_ack(response: oneshot::Receiver<Result<(), String>>) -> Result<(), String> {
    response
        .await
        .map_err(|_| "transfer queue actor dropped its durable acknowledgement".to_string())?
}

#[async_trait]
pub trait QueueClock: Send + Sync {
    fn now_ms(&self) -> u64;
    async fn sleep_until(&self, unix_ms: u64);
}

pub struct SystemQueueClock;

#[async_trait]
impl QueueClock for SystemQueueClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    async fn sleep_until(&self, unix_ms: u64) {
        let delay_ms = unix_ms.saturating_sub(self.now_ms());
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }
}

pub(crate) type SharedTransferJobRunner = Arc<dyn TransferJobRunner>;

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::RunnerReporter;
    use crate::remote::transfer_queue::events::RunnerEvent;

    #[tokio::test]
    async fn progress_burst_keeps_one_wake_and_converges_to_the_latest_value() {
        let job_id = Uuid::from_u128(91);
        let lease_id = Uuid::from_u128(92);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let reporter = RunnerReporter::new(job_id, lease_id, event_tx);

        for bytes in 1..=10_000 {
            reporter.progress(bytes, bytes * 2, Some(10_001 - bytes));
        }

        assert_eq!(event_rx.len(), 1, "a burst queues at most one actor wake");
        let RunnerEvent::ProgressReady {
            job_id: reported_job,
            lease_id: reported_lease,
            slot,
        } = event_rx.recv().await.unwrap()
        else {
            panic!("progress publishes a progress-ready wake")
        };
        assert_eq!(reported_job, job_id);
        assert_eq!(reported_lease, lease_id);
        let latest = slot.take_latest_and_release_wake().unwrap();
        assert_eq!(latest.bytes, 10_000);
        assert_eq!(latest.speed_bytes_per_second, 20_000);
        assert_eq!(latest.eta_seconds, Some(1));
        assert!(event_rx.is_empty());

        reporter.progress(10_001, 30_000, None);
        assert_eq!(event_rx.len(), 1, "draining re-arms exactly one wake");
        let RunnerEvent::ProgressReady { slot, .. } = event_rx.recv().await.unwrap() else {
            panic!("progress publishes a progress-ready wake")
        };
        assert_eq!(slot.take_latest_and_release_wake().unwrap().bytes, 10_001);
    }
}
