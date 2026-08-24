use std::{
    sync::Arc,
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

#[derive(Debug)]
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
        let _ = self.send(RunnerEvent::Progress {
            job_id: self.job_id,
            lease_id: self.lease_id,
            bytes,
            speed_bytes_per_second,
            eta_seconds,
        });
    }

    fn send(&self, event: RunnerEvent) -> Result<(), String> {
        self.event_tx
            .send(event)
            .map_err(|_| "transfer queue actor is unavailable".to_string())
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
