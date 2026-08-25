use std::{
    fmt,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use parking_lot::Mutex as ParkingMutex;
use termlab_remote::{
    error::RemoteError,
    transfer::{
        ControlDecision, CopyOutcome, SftpFileHandle, SftpSessionHandle, SourceFingerprint,
        copy::{CopyError, CopyStage, copy_with_checkpoint_typed},
        download_to_partial_pipelined, fingerprint_open_local, fingerprint_open_remote,
        open_local_partial, open_remote_partial, open_sftp_session,
        pipelined::PipelineTuning,
        truncate_local_partial, truncate_remote_partial, upload_to_partial_pipelined,
    },
};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{OnceCell, mpsc, oneshot, watch};
use uuid::Uuid;

use super::{
    artifacts::{ArtifactInventory, RecoveryAction, recovery_action},
    events::RunnerEvent,
    model::{
        AttentionReason, CommitPhase, CompletionResult, ConflictPolicy, ManagedArtifacts,
        TransferDirection, TransferEndpoint, TransferJob, TransferJobState,
    },
    scheduler::FailureClass,
};
use crate::remote::RemoteState;

#[derive(Clone)]
pub(crate) struct ResolvedSftpConnection {
    pub(crate) ssh_handle:
        Arc<termlab_remote::russh::client::Handle<termlab_remote::handler::TermLabSshHandler>>,
    sftp_session: Arc<OnceCell<Arc<SftpSessionHandle>>>,
}

async fn cached_resource<T, E, F, Fut>(cell: &OnceCell<Arc<T>>, initialize: F) -> Result<Arc<T>, E>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    cell.get_or_try_init(|| async { initialize().await.map(Arc::new) })
        .await
        .cloned()
}

impl ResolvedSftpConnection {
    async fn sftp_session(&self) -> Result<Arc<SftpSessionHandle>, TransferIoError> {
        cached_resource(&self.sftp_session, || open_sftp_session(&self.ssh_handle))
            .await
            .map_err(TransferIoError::from_remote)
    }
}

#[derive(Clone, Copy)]
struct LiveConnectionIdentity<'a> {
    key: &'a str,
    server_entry_id: Option<&'a str>,
    host: &'a str,
    port: u16,
    user: &'a str,
}

fn select_live_connection_key<'a>(
    connections: impl IntoIterator<Item = LiveConnectionIdentity<'a>>,
    endpoint: &TransferEndpoint,
) -> Option<&'a str> {
    connections.into_iter().find_map(|connection| {
        let matches = match endpoint {
            TransferEndpoint::Configured {
                server_entry_id, ..
            } => connection.server_entry_id == Some(server_entry_id.as_str()),
            TransferEndpoint::AdHoc {
                host, port, user, ..
            } => connection.host == host && connection.port == *port && connection.user == user,
        };
        matches.then_some(connection.key)
    })
}

pub(crate) fn resolve_live_sftp_connection(
    state: &RemoteState,
    endpoint: &TransferEndpoint,
) -> Option<ResolvedSftpConnection> {
    let key = select_live_connection_key(
        state
            .connections
            .iter()
            .map(|(key, connection)| LiveConnectionIdentity {
                key,
                server_entry_id: connection.server_entry_id.as_deref(),
                host: &connection.host,
                port: connection.port,
                user: &connection.user,
            }),
        endpoint,
    )?;
    state
        .connections
        .get(key)
        .map(|connection| ResolvedSftpConnection {
            ssh_handle: Arc::clone(&connection.ssh_handle),
            sftp_session: Arc::new(OnceCell::new()),
        })
}

trait ConnectionResolver: Send + Sync {
    type Connection: Clone + Send + Sync + 'static;

    fn resolve(&self, endpoint: &TransferEndpoint) -> Option<Self::Connection>;
}

struct LiveConnectionResolver {
    remote: Arc<ParkingMutex<RemoteState>>,
}

impl ConnectionResolver for LiveConnectionResolver {
    type Connection = ResolvedSftpConnection;

    fn resolve(&self, endpoint: &TransferEndpoint) -> Option<Self::Connection> {
        resolve_live_sftp_connection(&self.remote.lock(), endpoint)
    }
}

#[derive(Debug, Clone)]
struct TransferIoError {
    class: FailureClass,
    message: String,
    source_missing: bool,
}

impl TransferIoError {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Transient,
            message: message.into(),
            source_missing: false,
        }
    }

    fn permanent(message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Permanent,
            message: message.into(),
            source_missing: false,
        }
    }

    fn source_missing(message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Permanent,
            message: message.into(),
            source_missing: true,
        }
    }

    fn from_remote(error: RemoteError) -> Self {
        let error = match error {
            RemoteError::SourceNotFound(message) => return Self::source_missing(message),
            error => error,
        };
        let class = match &error {
            RemoteError::Connection(_) => FailureClass::Transient,
            RemoteError::Io(error) => classify_io_failure(error.kind()),
            RemoteError::Sftp(message) => classify_transport_failure(message),
            RemoteError::Transfer(_) => FailureClass::Permanent,
            RemoteError::SourceNotFound(_) => unreachable!("handled above"),
            RemoteError::Auth(_)
            | RemoteError::Tunnel(_)
            | RemoteError::KnownHosts(_)
            | RemoteError::Other(_) => FailureClass::Permanent,
        };
        let message = error.to_string();
        Self {
            class,
            message,
            source_missing: false,
        }
    }

    fn from_operation(class: FailureClass, operation: &str, error: impl fmt::Display) -> Self {
        Self {
            class,
            message: format!("{operation} failed: {error}"),
            source_missing: false,
        }
    }

    fn from_local(error: impl fmt::Display) -> Self {
        Self::permanent(error.to_string())
    }

    fn from_local_source(error: RemoteError) -> Self {
        match error {
            RemoteError::SourceNotFound(message) => Self::source_missing(message),
            error => Self::from_local(error),
        }
    }

    fn from_remote_operation(operation: &str, error: impl fmt::Display) -> Self {
        let error = error.to_string();
        let class = classify_transport_failure(&error);
        Self::from_operation(class, operation, error)
    }

    fn from_io_operation(operation: &str, error: std::io::Error) -> Self {
        let class = classify_io_failure(error.kind());
        Self::from_operation(class, operation, error)
    }

    fn from_copy(direction: &TransferDirection, error: CopyError) -> Self {
        let class = match &error {
            CopyError::Io { stage, kind, cause } if copy_stage_is_remote(direction, *stage) => {
                classify_remote_io_failure(*kind, cause)
            }
            CopyError::InvalidChunkSize
            | CopyError::OffsetBeyondSource { .. }
            | CopyError::Io { .. } => FailureClass::Permanent,
        };
        Self {
            class,
            message: error.to_string(),
            source_missing: false,
        }
    }
}

fn copy_stage_is_remote(direction: &TransferDirection, stage: CopyStage) -> bool {
    match direction {
        TransferDirection::Upload => matches!(
            stage,
            CopyStage::SeekDestination | CopyStage::WriteDestination
        ),
        TransferDirection::Download => {
            matches!(stage, CopyStage::SeekSource | CopyStage::ReadSource)
        }
    }
}

fn classify_remote_io_failure(kind: std::io::ErrorKind, raw_cause: &str) -> FailureClass {
    match kind {
        std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::TimedOut
        | std::io::ErrorKind::Interrupted
        | std::io::ErrorKind::UnexpectedEof => FailureClass::Transient,
        std::io::ErrorKind::Other => classify_transport_failure(raw_cause),
        _ => FailureClass::Permanent,
    }
}

fn classify_io_failure(kind: std::io::ErrorKind) -> FailureClass {
    match kind {
        std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::TimedOut
        | std::io::ErrorKind::Interrupted
        | std::io::ErrorKind::UnexpectedEof => FailureClass::Transient,
        _ => FailureClass::Permanent,
    }
}

/// Classify only the raw error supplied by remote transport infrastructure.
/// Operation context and user-controlled paths must be appended after this.
fn classify_transport_failure(error: &str) -> FailureClass {
    let error = error.to_ascii_lowercase();
    let cause = error.as_str();
    let explicit_disconnect = cause.starts_with("disconnect")
        || cause.contains(" disconnected")
        || cause.contains("connection lost");
    let explicit_timeout = cause == "timeout"
        || cause.starts_with("timeout ")
        || cause.contains("timed out")
        || cause.contains("connection timeout")
        || cause.contains("i/o timeout");
    if explicit_disconnect
        || explicit_timeout
        || [
            "connection reset",
            "connection aborted",
            "channel closed",
            "connection closed",
            "broken pipe",
            "not connected",
            "unexpected eof",
        ]
        .iter()
        .any(|indicator| cause.contains(indicator))
    {
        FailureClass::Transient
    } else {
        FailureClass::Permanent
    }
}

impl fmt::Display for TransferIoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug)]
enum PromotionError {
    DestinationExists { message: String },
    Ambiguous { message: String },
    Failed(TransferIoError),
}

impl PromotionError {
    fn message(&self) -> &str {
        match self {
            Self::DestinationExists { message } | Self::Ambiguous { message } => message,
            Self::Failed(error) => &error.message,
        }
    }

    fn allows_backup_restore(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    fn into_transfer_error(self) -> TransferIoError {
        match self {
            Self::DestinationExists { message } | Self::Ambiguous { message } => {
                TransferIoError::permanent(message)
            }
            Self::Failed(error) => error,
        }
    }
}

/// Everything a `copy_ranges` attempt needs besides the opened handles and
/// the control/progress callbacks.
#[derive(Clone, Copy)]
struct CopyRangesRequest<'a> {
    job: &'a TransferJob,
    artifacts: &'a ManagedArtifacts,
    /// The fingerprint this attempt established for the opened source. The
    /// pipelined engines reopen the source behind the runner's back, so they
    /// re-report a fingerprint that must match this one exactly.
    established: &'a SourceFingerprint,
    offset: u64,
    total: u64,
    tuning: PipelineTuning,
}

/// How a `copy_ranges` attempt failed, and whether the runner is allowed to
/// re-run it once sequentially.
#[derive(Debug)]
enum CopyRangesError {
    /// A real failure, already classified for the job's direction.
    Failed(TransferIoError),
    /// A pipelined attempt failed inside its first window — before any byte
    /// past `offset + depth * chunk_bytes` was ever reported durable — so no
    /// concurrency-dependent progress can be lost by retrying at depth 1.
    DegradeToSequential(TransferIoError),
    /// The source changed underneath the attempt after the runner had already
    /// established its fingerprint.
    SourceChanged {
        expected: SourceFingerprint,
        actual: SourceFingerprint,
    },
}

impl CopyRangesError {
    fn from_copy(direction: &TransferDirection, error: CopyError) -> Self {
        Self::Failed(TransferIoError::from_copy(direction, error))
    }

    /// Production degrades from a `RemoteError` the pipelined engine
    /// returned; tests build the marker straight from a typed copy failure.
    #[cfg(test)]
    fn degrade_to_sequential(direction: &TransferDirection, error: CopyError) -> Self {
        Self::DegradeToSequential(TransferIoError::from_copy(direction, error))
    }

    fn message(&self) -> String {
        match self {
            Self::Failed(error) | Self::DegradeToSequential(error) => error.message.clone(),
            Self::SourceChanged { expected, actual } => format!(
                "source changed during the attempt (size {} -> {})",
                expected.size, actual.size
            ),
        }
    }
}

/// Strip the `"<stage> failed: "` prefix [`CopyError::Io`] renders, so only
/// transport's own words are ever classified.
fn copy_stage_cause(message: &str) -> Option<&str> {
    for stage in [
        CopyStage::SeekSource,
        CopyStage::SeekDestination,
        CopyStage::ReadSource,
        CopyStage::WriteDestination,
    ] {
        let prefix = format!("{stage} failed: ");
        if let Some(cause) = message.strip_prefix(prefix.as_str()) {
            return Some(cause);
        }
    }
    None
}

/// Classify a failure one of the pipelined engines reported.
///
/// They flatten the typed [`CopyError`] into `RemoteError::Transfer`, which
/// [`TransferIoError::from_remote`] deliberately treats as permanent so a
/// user-controlled path can never fake a transient cause. A copy-stage prefix
/// proves the text really is transport's own, so a pipelined copy failure
/// keeps the same transient/permanent split the sequential engine gives it;
/// everything else falls back to the conservative mapping.
fn classify_pipelined_failure(error: RemoteError) -> TransferIoError {
    let RemoteError::Transfer(message) = &error else {
        return TransferIoError::from_remote(error);
    };
    match copy_stage_cause(message) {
        Some(cause) => TransferIoError {
            class: classify_transport_failure(cause),
            message: message.clone(),
            source_missing: false,
        },
        None => TransferIoError::from_remote(error),
    }
}

/// The last offset the pipelined engine's first window can reach.
///
/// A failure at or below this position cannot have depended on any chunk
/// completing out of order, so re-running the same range sequentially is
/// safe. `total` clamps it: when the whole remaining range fits inside one
/// window, every failure is a first-window failure.
fn pipeline_first_window_end(offset: u64, total: u64, tuning: PipelineTuning) -> u64 {
    let window = (tuning.depth as u64).saturating_mul(tuning.chunk_bytes as u64);
    offset.saturating_add(window).min(total)
}

/// The sequential copy engine behind [`TransferIo::copy_ranges`]'s default.
///
/// Kept as a free function so the trait default and the real IO's depth-1
/// path run byte-for-byte the same code.
async fn sequential_copy_ranges<S, P>(
    request: CopyRangesRequest<'_>,
    source: &mut S,
    partial: &mut P,
    control: &mut (dyn FnMut() -> ControlDecision + Send),
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<CopyOutcome, CopyRangesError>
where
    S: AsyncRead + AsyncSeek + Unpin + Send,
    P: AsyncWrite + AsyncSeek + Unpin + Send,
{
    copy_with_checkpoint_typed(
        source,
        partial,
        request.offset,
        request.total,
        request.tuning.chunk_bytes.max(1),
        control,
        progress,
    )
    .await
    .map_err(|error| CopyRangesError::from_copy(&request.job.direction, error))
}

#[async_trait]
trait TransferIo<C>: Send + Sync {
    type Source: AsyncRead + AsyncSeek + Unpin + Send;
    type Partial: AsyncWrite + AsyncSeek + Unpin + Send;

    /// Copy `offset..total` from the opened source into the opened partial.
    ///
    /// The default is the sequential engine, so every implementation keeps
    /// today's behavior for free; [`RealTransferIo`] overrides it to drive
    /// the pipelined engine over its own positional handles when the tuning
    /// asks for depth greater than one.
    async fn copy_ranges(
        &self,
        _connection: &C,
        request: CopyRangesRequest<'_>,
        source: &mut Self::Source,
        partial: &mut Self::Partial,
        control: &mut (dyn FnMut() -> ControlDecision + Send),
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<CopyOutcome, CopyRangesError> {
        sequential_copy_ranges(request, source, partial, control, progress).await
    }

    async fn open_source(
        &self,
        connection: &C,
        job: &TransferJob,
    ) -> Result<(Self::Source, SourceFingerprint), TransferIoError>;
    async fn inventory(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<ArtifactInventory, TransferIoError>;
    async fn partial_size(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<Option<u64>, TransferIoError>;
    async fn truncate_partial(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
        bytes: u64,
    ) -> Result<(), TransferIoError>;
    async fn open_partial(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
        resume: bool,
    ) -> Result<Self::Partial, TransferIoError>;
    async fn finish_source(&self, source: &mut Self::Source) -> Result<(), TransferIoError>;
    async fn finish_partial(&self, partial: &mut Self::Partial) -> Result<(), TransferIoError>;
    async fn move_final_to_backup(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError>;
    async fn promote_partial_no_replace(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), PromotionError>;
    async fn restore_backup(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError>;
    async fn delete_backup(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError>;
    async fn cleanup_owned_artifacts(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError>;
}

#[async_trait]
trait SftpAttempt: Send + Sync {
    async fn run(
        &self,
        job: TransferJob,
        control: RunnerControl,
        reporter: RunnerReporter,
        tuning: PipelineTuning,
    ) -> RunnerResult;
}

struct RunnerServices<R, I> {
    resolver: R,
    io: I,
}

pub(crate) struct SftpTransferJobRunner {
    attempt: Arc<dyn SftpAttempt>,
}

impl SftpTransferJobRunner {
    pub(crate) fn new(remote: Arc<ParkingMutex<RemoteState>>) -> Self {
        Self::with_services(LiveConnectionResolver { remote }, RealTransferIo::new())
    }

    #[cfg(test)]
    fn new_with_partial_seek_observer(
        remote: Arc<ParkingMutex<RemoteState>>,
        observer: PartialSeekObserver,
    ) -> Self {
        Self::with_services(
            LiveConnectionResolver { remote },
            RealTransferIo::with_partial_seek_observer(observer),
        )
    }

    fn with_services<R, I>(resolver: R, io: I) -> Self
    where
        R: ConnectionResolver + 'static,
        I: TransferIo<R::Connection> + 'static,
    {
        Self {
            attempt: Arc::new(RunnerServices { resolver, io }),
        }
    }
}

#[async_trait]
impl TransferJobRunner for SftpTransferJobRunner {
    async fn run(
        &self,
        job: TransferJob,
        control: RunnerControl,
        reporter: RunnerReporter,
        tuning: PipelineTuning,
    ) -> RunnerResult {
        self.attempt.run(job, control, reporter, tuning).await
    }
}

#[async_trait]
impl<R, I> SftpAttempt for RunnerServices<R, I>
where
    R: ConnectionResolver,
    I: TransferIo<R::Connection>,
{
    async fn run(
        &self,
        job: TransferJob,
        control: RunnerControl,
        reporter: RunnerReporter,
        tuning: PipelineTuning,
    ) -> RunnerResult {
        if control.state() == RunnerControlState::Cancel
            && matches!(
                job.state,
                TransferJobState::Queued
                    | TransferJobState::Paused
                    | TransferJobState::NeedsConnection { .. }
                    | TransferJobState::NeedsAttention { .. }
            )
        {
            let artifacts = match expected_artifacts(&job) {
                Ok(artifacts) => artifacts,
                Err(message) => {
                    return RunnerResult::Cancelled {
                        cleanup_error: Some(format!(
                            "managed transfer artifacts could not be resolved: {message}"
                        )),
                    };
                }
            };
            if job.artifacts.as_ref() != Some(&artifacts) {
                return RunnerResult::NeedsAttention(AttentionReason::CommitRecovery {
                    message:
                        "persisted managed artifacts are missing or do not match the current destination; all artifacts were preserved"
                            .into(),
                });
            }
            let Some(connection) = self.resolver.resolve(&job.endpoint) else {
                return RunnerResult::Cancelled {
                    cleanup_error: Some(
                        "managed artifacts were retained because no matching live SSH connection was available"
                            .into(),
                    ),
                };
            };
            return match cancel_inactive_artifacts(&self.io, &connection, &job, &artifacts).await {
                Ok(()) => RunnerResult::Cancelled {
                    cleanup_error: None,
                },
                Err(reason) => RunnerResult::NeedsAttention(reason),
            };
        }
        let Some(connection) = self.resolver.resolve(&job.endpoint) else {
            return RunnerResult::NeedsConnection(
                "No matching live SSH connection; reconnect the transfer endpoint".into(),
            );
        };
        if let Err(message) = reporter.checking().await {
            return permanent_failure(message);
        }
        let artifacts = match expected_artifacts(&job) {
            Ok(artifacts) => artifacts,
            Err(message) => return permanent_failure(message),
        };
        if job.durable_checkpoint > 0
            && (job
                .source_fingerprint
                .as_ref()
                .and_then(|fingerprint| fingerprint.modified_token.as_ref())
                .is_none()
                || job.artifacts.as_ref() != Some(&artifacts))
        {
            return RunnerResult::NeedsAttention(AttentionReason::SourceCannotResume);
        }
        if let Some(persisted) = &job.artifacts
            && persisted != &artifacts
        {
            return RunnerResult::NeedsAttention(AttentionReason::CommitRecovery {
                message: "persisted managed artifacts do not match the current destination; preserve all artifacts and restart or rename explicitly".into(),
            });
        }

        if job.commit_phase != CommitPhase::None {
            return recover_commit(&self.io, &connection, &job, &artifacts, &reporter).await;
        }

        let (mut source, actual) = match self.io.open_source(&connection, &job).await {
            Ok(opened) => opened,
            Err(error) => return failed(error),
        };
        if job.durable_checkpoint > 0 && actual.modified_token.is_none() {
            let _ = self.io.finish_source(&mut source).await;
            return RunnerResult::NeedsAttention(AttentionReason::SourceCannotResume);
        }
        if let Some(expected) = &job.source_fingerprint
            && expected != &actual
        {
            let _ = self.io.finish_source(&mut source).await;
            return RunnerResult::NeedsAttention(AttentionReason::SourceChanged {
                expected: expected.clone(),
                actual,
            });
        }
        if let Err(message) = reporter
            .fingerprinted(actual.clone(), actual.size, artifacts.clone())
            .await
        {
            let _ = self.io.finish_source(&mut source).await;
            return permanent_failure(message);
        }

        let inventory = match self.io.inventory(&connection, &job, &artifacts).await {
            Ok(inventory) => inventory,
            Err(error) => {
                let _ = self.io.finish_source(&mut source).await;
                return failed(error);
            }
        };
        if inventory.backup_exists {
            let _ = self.io.finish_source(&mut source).await;
            return RunnerResult::NeedsAttention(AttentionReason::CommitRecovery {
                message: format!(
                    "backup exists before commit with persisted None phase ({inventory}); preserve all artifacts and resolve explicitly"
                ),
            });
        }
        let resume_available = inventory.partial_exists
            && job.durable_checkpoint > 0
            && job.artifacts.as_ref() == Some(&artifacts)
            && job.source_fingerprint.as_ref() == Some(&actual)
            && actual.modified_token.is_some();
        if inventory.final_exists && job.conflict_policy == ConflictPolicy::Ask {
            let _ = self.io.finish_source(&mut source).await;
            return RunnerResult::NeedsAttention(AttentionReason::DestinationConflict {
                resume_available,
            });
        }

        if job.durable_checkpoint > 0 && !inventory.partial_exists {
            let _ = self.io.finish_source(&mut source).await;
            return RunnerResult::NeedsAttention(AttentionReason::MissingPartial);
        }

        let mut offset = job.durable_checkpoint;
        if inventory.partial_exists {
            let partial_size = match self.io.partial_size(&connection, &job, &artifacts).await {
                Ok(Some(size)) => size,
                Ok(None) => {
                    let _ = self.io.finish_source(&mut source).await;
                    return RunnerResult::NeedsAttention(AttentionReason::MissingPartial);
                }
                Err(error) => {
                    let _ = self.io.finish_source(&mut source).await;
                    return failed(error);
                }
            };
            if partial_size > offset {
                if let Err(error) = self
                    .io
                    .truncate_partial(&connection, &job, &artifacts, offset)
                    .await
                {
                    let _ = self.io.finish_source(&mut source).await;
                    return failed(error);
                }
            } else if partial_size < offset {
                if let Err(message) = reporter.durable_checkpoint(partial_size).await {
                    let _ = self.io.finish_source(&mut source).await;
                    return permanent_failure(message);
                }
                offset = partial_size;
            }
        }

        let mut partial = match self
            .io
            .open_partial(&connection, &job, &artifacts, offset > 0)
            .await
        {
            Ok(partial) => partial,
            Err(error) => {
                let _ = self.io.finish_source(&mut source).await;
                return failed(error);
            }
        };
        let mut control_fn = || match control.state() {
            RunnerControlState::Run => ControlDecision::Continue,
            RunnerControlState::Pause => ControlDecision::Pause,
            RunnerControlState::Cancel => ControlDecision::Cancel,
        };
        let mut progress_fn = |bytes: u64, _total: u64| reporter.progress(bytes, None, None);
        let request = CopyRangesRequest {
            job: &job,
            artifacts: &artifacts,
            established: &actual,
            offset,
            total: actual.size,
            tuning,
        };
        let mut copy_result = self
            .io
            .copy_ranges(
                &connection,
                request,
                &mut source,
                &mut partial,
                &mut control_fn,
                &mut progress_fn,
            )
            .await;
        if let Err(CopyRangesError::DegradeToSequential(_)) = &copy_result {
            // Nothing past the first window was ever reported, so the same
            // range can simply be re-copied sequentially over the handles this
            // attempt already opened: both engines address absolute offsets,
            // so the retry overwrites rather than appends.
            if let Err(error) = &copy_result {
                log::warn!(
                    "pipelined transfer for job {} degraded to sequential: {}",
                    job.id,
                    error.message()
                );
            }
            copy_result = self
                .io
                .copy_ranges(
                    &connection,
                    CopyRangesRequest {
                        tuning: PipelineTuning { depth: 1, ..tuning },
                        ..request
                    },
                    &mut source,
                    &mut partial,
                    &mut control_fn,
                    &mut progress_fn,
                )
                .await;
        }

        let source_finish = self.io.finish_source(&mut source).await;
        let partial_finish = self.io.finish_partial(&mut partial).await;
        let outcome = match copy_result {
            Ok(outcome) => outcome,
            Err(CopyRangesError::SourceChanged { expected, actual }) => {
                return RunnerResult::NeedsAttention(AttentionReason::SourceChanged {
                    expected,
                    actual,
                });
            }
            Err(CopyRangesError::Failed(error) | CopyRangesError::DegradeToSequential(error)) => {
                return failed(error);
            }
        };
        if let Err(error) = source_finish.and(partial_finish) {
            return failed(error);
        }

        let bytes = match outcome {
            CopyOutcome::Completed { bytes }
            | CopyOutcome::Paused { bytes }
            | CopyOutcome::Cancelled { bytes } => bytes,
        };
        if let Err(message) = reporter.durable_checkpoint(bytes).await {
            return permanent_failure(message);
        }
        match outcome {
            CopyOutcome::Paused { .. } => RunnerResult::Paused {
                durable_checkpoint: bytes,
            },
            CopyOutcome::Cancelled { .. } => {
                let cleanup_error = self
                    .io
                    .cleanup_owned_artifacts(&connection, &job, &artifacts)
                    .await
                    .err()
                    .map(|error| error.message);
                RunnerResult::Cancelled { cleanup_error }
            }
            CopyOutcome::Completed { .. } if bytes != actual.size => permanent_failure(format!(
                "opened source ended at {bytes} bytes but its fingerprint declared {} bytes",
                actual.size
            )),
            CopyOutcome::Completed { .. } => {
                commit_fresh(&self.io, &connection, &job, &artifacts, &actual, &reporter).await
            }
        }
    }
}

async fn cancel_inactive_artifacts<C, I>(
    io: &I,
    connection: &C,
    job: &TransferJob,
    artifacts: &ManagedArtifacts,
) -> Result<(), AttentionReason>
where
    C: Send + Sync,
    I: TransferIo<C>,
{
    let inventory = io
        .inventory(connection, job, artifacts)
        .await
        .map_err(cleanup_attention)?;
    // The shared recovery policy is the authority for which layouts are
    // provably recoverable. Cancellation rolls back to an owned backup when
    // one exists; otherwise it preserves the current final or promotes the
    // sole complete partial before removing any remaining managed artifacts.
    match recovery_action(job.commit_phase, job.commit_backup_expected, inventory) {
        RecoveryAction::ResumeCopy | RecoveryAction::MoveFinalToBackup => io
            .cleanup_owned_artifacts(connection, job, artifacts)
            .await
            .map_err(cleanup_attention),
        RecoveryAction::PromotePartial if inventory.backup_exists => {
            io.restore_backup(connection, job, artifacts)
                .await
                .map_err(cleanup_attention)?;
            io.cleanup_owned_artifacts(connection, job, artifacts)
                .await
                .map_err(cleanup_attention)
        }
        RecoveryAction::PromotePartial => io
            .promote_partial_no_replace(connection, job, artifacts)
            .await
            .map_err(PromotionError::into_transfer_error)
            .map_err(cleanup_attention),
        RecoveryAction::RestoreBackup => {
            io.restore_backup(connection, job, artifacts)
                .await
                .map_err(cleanup_attention)?;
            io.cleanup_owned_artifacts(connection, job, artifacts)
                .await
                .map_err(cleanup_attention)
        }
        RecoveryAction::DeleteBackupAndComplete => io
            .delete_backup(connection, job, artifacts)
            .await
            .map_err(cleanup_attention),
        RecoveryAction::Complete => Ok(()),
        RecoveryAction::NeedsAttention { message } => {
            Err(AttentionReason::CommitRecovery { message })
        }
    }
}

fn cleanup_attention(error: TransferIoError) -> AttentionReason {
    AttentionReason::Cleanup {
        message: error.message,
    }
}

fn expected_artifacts(job: &TransferJob) -> Result<ManagedArtifacts, String> {
    match job.direction {
        TransferDirection::Upload => ManagedArtifacts::for_destination(job.id, &job.remote_path),
        TransferDirection::Download => {
            ManagedArtifacts::for_local_destination(job.id, Path::new(&job.local_path))
        }
    }
    .map_err(|error| error.to_string())
}

fn failed(error: TransferIoError) -> RunnerResult {
    if error.source_missing {
        RunnerResult::NeedsAttention(AttentionReason::SourceMissing)
    } else {
        RunnerResult::Failed {
            class: error.class,
            message: error.message,
        }
    }
}

fn permanent_failure(message: impl Into<String>) -> RunnerResult {
    RunnerResult::Failed {
        class: FailureClass::Permanent,
        message: message.into(),
    }
}

async fn commit_fresh<C, I>(
    io: &I,
    connection: &C,
    job: &TransferJob,
    artifacts: &ManagedArtifacts,
    source_fingerprint: &SourceFingerprint,
    reporter: &RunnerReporter,
) -> RunnerResult
where
    C: Send + Sync,
    I: TransferIo<C>,
{
    let inventory = match io.inventory(connection, job, artifacts).await {
        Ok(inventory) => inventory,
        Err(error) => return failed(error),
    };
    if inventory.backup_exists {
        return RunnerResult::NeedsAttention(AttentionReason::CommitRecovery {
            message: format!(
                "backup appeared immediately before commit with persisted None phase ({inventory}); preserve all artifacts and resolve explicitly"
            ),
        });
    }
    if inventory.final_exists && job.conflict_policy == ConflictPolicy::Ask {
        return RunnerResult::NeedsAttention(AttentionReason::DestinationConflict {
            resume_available: source_fingerprint.modified_token.is_some(),
        });
    }
    let final_exists = inventory.final_exists;
    if let Err(message) = reporter.commit_prepared(final_exists).await {
        return permanent_failure(message);
    }
    if final_exists && let Err(error) = io.move_final_to_backup(connection, job, artifacts).await {
        return commit_attention(io, connection, job, artifacts, error.message).await;
    }
    if let Err(message) = reporter.commit_phase(CommitPhase::BackupMoved).await {
        return permanent_failure(message);
    }
    if let Err(error) = io
        .promote_partial_no_replace(connection, job, artifacts)
        .await
    {
        if matches!(error, PromotionError::DestinationExists { .. })
            && job.conflict_policy == ConflictPolicy::Ask
        {
            return RunnerResult::NeedsAttention(AttentionReason::DestinationConflict {
                resume_available: source_fingerprint.modified_token.is_some(),
            });
        }
        if final_exists && error.allows_backup_restore() {
            let _ = io.restore_backup(connection, job, artifacts).await;
        }
        return commit_attention(io, connection, job, artifacts, error.message()).await;
    }
    if let Err(message) = reporter.commit_phase(CommitPhase::PartialPromoted).await {
        return permanent_failure(message);
    }
    if final_exists && let Err(error) = io.delete_backup(connection, job, artifacts).await {
        return commit_attention(io, connection, job, artifacts, error.message).await;
    }
    if let Err(message) = reporter.commit_phase(CommitPhase::Complete).await {
        return permanent_failure(message);
    }
    RunnerResult::Completed(CompletionResult::Transferred)
}

async fn recover_commit<C, I>(
    io: &I,
    connection: &C,
    job: &TransferJob,
    artifacts: &ManagedArtifacts,
    reporter: &RunnerReporter,
) -> RunnerResult
where
    C: Send + Sync,
    I: TransferIo<C>,
{
    let mut phase = job.commit_phase;
    let mut backup_expected = job.commit_backup_expected;

    loop {
        let inventory = match io.inventory(connection, job, artifacts).await {
            Ok(inventory) => inventory,
            Err(error) => return failed(error),
        };

        // A commit that began against a fresh destination may encounter one
        // late final before promotion. Explicit Overwrite authorization lets
        // that late final become the backup. This extension never applies when
        // an original backup was promised; a missing authoritative overwrite
        // backup remains conservative.
        if phase == CommitPhase::BackupMoved
            && backup_expected == Some(false)
            && inventory.partial_exists
            && inventory.final_exists
            && !inventory.backup_exists
            && job.conflict_policy == ConflictPolicy::Overwrite
        {
            if let Err(error) = io.move_final_to_backup(connection, job, artifacts).await {
                return commit_attention(io, connection, job, artifacts, error.message).await;
            }
            if let Err(message) = reporter
                .commit_phase_with_backup_expectation(CommitPhase::BackupMoved, Some(true))
                .await
            {
                return permanent_failure(message);
            }
            backup_expected = Some(true);
            continue;
        }

        let action = recovery_action(phase, backup_expected, inventory);
        match action {
            RecoveryAction::ResumeCopy => {
                return commit_attention(
                    io,
                    connection,
                    job,
                    artifacts,
                    format!("commit recovery cannot resume copy from persisted phase {phase:?}"),
                )
                .await;
            }
            RecoveryAction::MoveFinalToBackup => {
                if job.conflict_policy == ConflictPolicy::Ask {
                    return RunnerResult::NeedsAttention(AttentionReason::DestinationConflict {
                        resume_available: job
                            .source_fingerprint
                            .as_ref()
                            .and_then(|fingerprint| fingerprint.modified_token.as_ref())
                            .is_some(),
                    });
                }
                if let Err(error) = io.move_final_to_backup(connection, job, artifacts).await {
                    return commit_attention(io, connection, job, artifacts, error.message).await;
                }
                if let Err(message) = reporter.commit_phase(CommitPhase::BackupMoved).await {
                    return permanent_failure(message);
                }
                phase = CommitPhase::BackupMoved;
            }
            RecoveryAction::PromotePartial => {
                if phase == CommitPhase::Prepared {
                    if let Err(message) = reporter.commit_phase(CommitPhase::BackupMoved).await {
                        return permanent_failure(message);
                    }
                    phase = CommitPhase::BackupMoved;
                    continue;
                }
                if let Err(error) = io
                    .promote_partial_no_replace(connection, job, artifacts)
                    .await
                {
                    if inventory.backup_exists && error.allows_backup_restore() {
                        let _ = io.restore_backup(connection, job, artifacts).await;
                    }
                    return commit_attention(io, connection, job, artifacts, error.message()).await;
                }
                if let Err(message) = reporter.commit_phase(CommitPhase::PartialPromoted).await {
                    return permanent_failure(message);
                }
                phase = CommitPhase::PartialPromoted;
            }
            RecoveryAction::RestoreBackup => {
                if let Err(error) = io.restore_backup(connection, job, artifacts).await {
                    return commit_attention(io, connection, job, artifacts, error.message).await;
                }
                return commit_attention(
                    io,
                    connection,
                    job,
                    artifacts,
                    format!("restored the owned backup required by the {phase:?} recovery policy"),
                )
                .await;
            }
            RecoveryAction::DeleteBackupAndComplete => {
                if let Err(error) = io.delete_backup(connection, job, artifacts).await {
                    return commit_attention(io, connection, job, artifacts, error.message).await;
                }
                if let Err(message) = reporter.commit_phase(CommitPhase::Complete).await {
                    return permanent_failure(message);
                }
                return RunnerResult::Completed(CompletionResult::Transferred);
            }
            RecoveryAction::Complete => {
                if phase != CommitPhase::Complete
                    && let Err(message) = reporter.commit_phase(CommitPhase::Complete).await
                {
                    return permanent_failure(message);
                }
                return RunnerResult::Completed(CompletionResult::Transferred);
            }
            RecoveryAction::NeedsAttention { message } => {
                return commit_attention(io, connection, job, artifacts, message).await;
            }
        }
    }
}

async fn commit_attention<C, I>(
    io: &I,
    connection: &C,
    job: &TransferJob,
    artifacts: &ManagedArtifacts,
    message: impl Into<String>,
) -> RunnerResult
where
    C: Send + Sync,
    I: TransferIo<C>,
{
    let message = message.into();
    let inventory = io
        .inventory(connection, job, artifacts)
        .await
        .map(|inventory| inventory.to_string())
        .unwrap_or_else(|error| format!("inventory unavailable: {}", error.message));
    RunnerResult::NeedsAttention(AttentionReason::CommitRecovery {
        message: format!("{message}; resulting artifact inventory: {inventory}"),
    })
}

#[cfg(test)]
// Test-only telemetry injected into the ordinary real-I/O runner. It is not a
// queue event, job field, or persisted schema value.
type PartialSeekObserver = Arc<dyn Fn(u64) + Send + Sync>;

struct RealTransferIo {
    #[cfg(test)]
    partial_seek_observer: Option<PartialSeekObserver>,
}

impl RealTransferIo {
    fn new() -> Self {
        Self {
            #[cfg(test)]
            partial_seek_observer: None,
        }
    }

    #[cfg(test)]
    fn with_partial_seek_observer(observer: PartialSeekObserver) -> Self {
        Self {
            partial_seek_observer: Some(observer),
        }
    }

    fn local_partial(&self, file: tokio::fs::File) -> RealPartial {
        RealPartial {
            handle: RealPartialHandle::Local(file),
            #[cfg(test)]
            seek_observer: self.partial_seek_observer.clone(),
            #[cfg(test)]
            seek_observation_pending: false,
        }
    }

    fn remote_partial(&self, file: SftpFileHandle) -> RealPartial {
        RealPartial {
            handle: RealPartialHandle::Remote(file),
            #[cfg(test)]
            seek_observer: self.partial_seek_observer.clone(),
            #[cfg(test)]
            seek_observation_pending: false,
        }
    }
}

enum RealSource {
    Local(tokio::fs::File),
    Remote(SftpFileHandle),
}

enum RealPartialHandle {
    Local(tokio::fs::File),
    Remote(SftpFileHandle),
}

struct RealPartial {
    handle: RealPartialHandle,
    #[cfg(test)]
    seek_observer: Option<PartialSeekObserver>,
    #[cfg(test)]
    seek_observation_pending: bool,
}

#[cfg(test)]
impl RealPartial {
    fn local_with_test_seek_observer(file: tokio::fs::File, observer: PartialSeekObserver) -> Self {
        Self {
            handle: RealPartialHandle::Local(file),
            seek_observer: Some(observer),
            seek_observation_pending: false,
        }
    }
}

impl AsyncRead for RealSource {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Local(file) => Pin::new(file).poll_read(context, buffer),
            Self::Remote(file) => Pin::new(file).poll_read(context, buffer),
        }
    }
}

impl AsyncSeek for RealSource {
    fn start_seek(self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        match self.get_mut() {
            Self::Local(file) => Pin::new(file).start_seek(position),
            Self::Remote(file) => Pin::new(file).start_seek(position),
        }
    }

    fn poll_complete(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<u64>> {
        match self.get_mut() {
            Self::Local(file) => Pin::new(file).poll_complete(context),
            Self::Remote(file) => Pin::new(file).poll_complete(context),
        }
    }
}

impl AsyncWrite for RealPartial {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut self.get_mut().handle {
            RealPartialHandle::Local(file) => Pin::new(file).poll_write(context, buffer),
            RealPartialHandle::Remote(file) => Pin::new(file).poll_write(context, buffer),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut self.get_mut().handle {
            RealPartialHandle::Local(file) => Pin::new(file).poll_flush(context),
            RealPartialHandle::Remote(file) => Pin::new(file).poll_flush(context),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut self.get_mut().handle {
            RealPartialHandle::Local(file) => Pin::new(file).poll_shutdown(context),
            RealPartialHandle::Remote(file) => Pin::new(file).poll_shutdown(context),
        }
    }
}

impl AsyncSeek for RealPartial {
    fn start_seek(self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        let this = self.get_mut();
        let result = match &mut this.handle {
            RealPartialHandle::Local(file) => Pin::new(file).start_seek(position),
            RealPartialHandle::Remote(file) => Pin::new(file).start_seek(position),
        };
        #[cfg(test)]
        if result.is_ok() {
            this.seek_observation_pending = true;
        }
        result
    }

    fn poll_complete(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<u64>> {
        let this = self.get_mut();
        let result = match &mut this.handle {
            RealPartialHandle::Local(file) => Pin::new(file).poll_complete(context),
            RealPartialHandle::Remote(file) => Pin::new(file).poll_complete(context),
        };
        #[cfg(test)]
        if let Poll::Ready(Ok(position)) = &result
            && this.seek_observation_pending
        {
            this.seek_observation_pending = false;
            if let Some(observer) = &this.seek_observer {
                observer(*position);
            }
        }
        result
    }
}

#[async_trait]
impl TransferIo<ResolvedSftpConnection> for RealTransferIo {
    type Source = RealSource;
    type Partial = RealPartial;

    async fn open_source(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
    ) -> Result<(Self::Source, SourceFingerprint), TransferIoError> {
        match job.direction {
            TransferDirection::Upload => fingerprint_open_local(&job.local_path)
                .await
                .map(|(file, fingerprint)| (RealSource::Local(file), fingerprint))
                .map_err(TransferIoError::from_local_source),
            TransferDirection::Download => {
                let session = connection.sftp_session().await?;
                fingerprint_open_remote(&session, &job.remote_path)
                    .await
                    .map(|(file, fingerprint)| (RealSource::Remote(file), fingerprint))
                    .map_err(TransferIoError::from_remote)
            }
        }
    }

    async fn inventory(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<ArtifactInventory, TransferIoError> {
        match job.direction {
            TransferDirection::Upload => {
                let session = connection.sftp_session().await?;
                let (final_exists, partial_exists, backup_exists) = tokio::try_join!(
                    remote_exists(&session, &job.remote_path),
                    remote_exists(&session, &artifacts.partial_path),
                    remote_exists(&session, &artifacts.backup_path),
                )?;
                Ok(ArtifactInventory {
                    final_exists,
                    partial_exists,
                    backup_exists,
                })
            }
            TransferDirection::Download => Ok(ArtifactInventory {
                final_exists: local_exists(&job.local_path).await?,
                partial_exists: local_exists(&artifacts.partial_path).await?,
                backup_exists: local_exists(&artifacts.backup_path).await?,
            }),
        }
    }

    async fn partial_size(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<Option<u64>, TransferIoError> {
        match job.direction {
            TransferDirection::Upload => {
                let session = connection.sftp_session().await?;
                match session.metadata(artifacts.partial_path.clone()).await {
                    Ok(metadata) => Ok(metadata.size),
                    Err(error) => match remote_exists(&session, &artifacts.partial_path).await {
                        Ok(false) => Ok(None),
                        Ok(true) => Err(TransferIoError::from_remote_operation(
                            "stat remote partial",
                            error,
                        )),
                        Err(probe_error) => Err(probe_error),
                    },
                }
            }
            TransferDirection::Download => match tokio::fs::metadata(&artifacts.partial_path).await
            {
                Ok(metadata) => Ok(Some(metadata.len())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(TransferIoError::permanent(format!(
                    "stat local partial failed: {error}"
                ))),
            },
        }
    }

    async fn truncate_partial(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
        bytes: u64,
    ) -> Result<(), TransferIoError> {
        match job.direction {
            TransferDirection::Upload => {
                let session = connection.sftp_session().await?;
                truncate_remote_partial(&session, &artifacts.partial_path, bytes)
                    .await
                    .map_err(TransferIoError::from_remote)
            }
            TransferDirection::Download => truncate_local_partial(&artifacts.partial_path, bytes)
                .await
                .map_err(TransferIoError::from_local),
        }
    }

    async fn open_partial(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
        resume: bool,
    ) -> Result<Self::Partial, TransferIoError> {
        match job.direction {
            TransferDirection::Upload => {
                let session = connection.sftp_session().await?;
                open_remote_partial(&session, &artifacts.partial_path, resume)
                    .await
                    .map(|file| self.remote_partial(file))
                    .map_err(TransferIoError::from_remote)
            }
            TransferDirection::Download => open_local_partial(&artifacts.partial_path, resume)
                .await
                .map(|file| self.local_partial(file))
                .map_err(TransferIoError::from_local),
        }
    }

    async fn copy_ranges(
        &self,
        connection: &ResolvedSftpConnection,
        request: CopyRangesRequest<'_>,
        source: &mut Self::Source,
        partial: &mut Self::Partial,
        control: &mut (dyn FnMut() -> ControlDecision + Send),
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<CopyOutcome, CopyRangesError> {
        if request.tuning.depth <= 1 {
            return sequential_copy_ranges(request, source, partial, control, progress).await;
        }

        let job = request.job;
        // The pipelined engines reopen the source themselves, so they report a
        // fresh fingerprint. This attempt already established one and told the
        // queue about it; anything else means the source moved underneath us.
        let expected = request.established.clone();
        let observed_change: Arc<Mutex<Option<SourceFingerprint>>> = Arc::new(Mutex::new(None));
        let change_sink = Arc::clone(&observed_change);
        let verify_fingerprint = move |current: SourceFingerprint| {
            let change_sink = Arc::clone(&change_sink);
            let expected = expected.clone();
            async move {
                if current == expected {
                    return Ok(());
                }
                let message = format!(
                    "source changed during the pipelined transfer (size {} -> {})",
                    expected.size, current.size
                );
                *change_sink
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(current);
                Err(RemoteError::Transfer(message))
            }
        };

        let mut furthest = request.offset;
        let result = {
            let mut tracked_progress = |bytes: u64, total: u64| {
                furthest = furthest.max(bytes);
                progress(bytes, total);
            };
            match job.direction {
                TransferDirection::Upload => {
                    upload_to_partial_pipelined(
                        &connection.ssh_handle,
                        &job.local_path,
                        &request.artifacts.partial_path,
                        request.offset,
                        request.tuning,
                        verify_fingerprint,
                        control,
                        &mut tracked_progress,
                    )
                    .await
                }
                TransferDirection::Download => {
                    download_to_partial_pipelined(
                        &connection.ssh_handle,
                        &job.remote_path,
                        &request.artifacts.partial_path,
                        request.offset,
                        request.tuning,
                        verify_fingerprint,
                        control,
                        &mut tracked_progress,
                    )
                    .await
                }
            }
        };

        let error = match result {
            Ok(outcome) => return Ok(outcome),
            Err(error) => error,
        };
        if let Some(actual) = observed_change
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            return Err(CopyRangesError::SourceChanged {
                expected: request.established.clone(),
                actual,
            });
        }
        let classified = classify_pipelined_failure(error);
        if furthest <= pipeline_first_window_end(request.offset, request.total, request.tuning) {
            return Err(CopyRangesError::DegradeToSequential(classified));
        }
        Err(CopyRangesError::Failed(classified))
    }

    async fn finish_source(&self, source: &mut Self::Source) -> Result<(), TransferIoError> {
        if let RealSource::Remote(file) = source {
            file.shutdown().await.map_err(|error| {
                TransferIoError::from_io_operation("close remote source", error)
            })?;
        }
        Ok(())
    }

    async fn finish_partial(&self, partial: &mut Self::Partial) -> Result<(), TransferIoError> {
        match &mut partial.handle {
            RealPartialHandle::Local(file) => {
                file.flush().await.map_err(|error| {
                    TransferIoError::permanent(format!("flush local partial failed: {error}"))
                })?;
                file.sync_all().await.map_err(|error| {
                    TransferIoError::permanent(format!("sync local partial failed: {error}"))
                })
            }
            RealPartialHandle::Remote(file) => {
                file.flush().await.map_err(|error| {
                    TransferIoError::from_io_operation("flush remote partial", error)
                })?;
                file.sync_all().await.map_err(|error| {
                    TransferIoError::from_remote_operation("sync remote partial", error)
                })?;
                file.shutdown().await.map_err(|error| {
                    TransferIoError::from_io_operation("close remote partial", error)
                })
            }
        }
    }

    async fn move_final_to_backup(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError> {
        rename_artifact(connection, job, final_path(job), &artifacts.backup_path).await
    }

    async fn promote_partial_no_replace(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), PromotionError> {
        rename_artifact_no_replace(connection, job, &artifacts.partial_path, final_path(job)).await
    }

    async fn restore_backup(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError> {
        rename_artifact_no_replace(connection, job, &artifacts.backup_path, final_path(job))
            .await
            .map_err(PromotionError::into_transfer_error)
    }

    async fn delete_backup(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError> {
        remove_artifact(connection, job, &artifacts.backup_path, false).await
    }

    async fn cleanup_owned_artifacts(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError> {
        let partial = remove_artifact(connection, job, &artifacts.partial_path, true).await;
        let backup = remove_artifact(connection, job, &artifacts.backup_path, true).await;
        partial.and(backup)
    }
}

fn final_path(job: &TransferJob) -> &str {
    match job.direction {
        TransferDirection::Upload => &job.remote_path,
        TransferDirection::Download => &job.local_path,
    }
}

async fn local_exists(path: &str) -> Result<bool, TransferIoError> {
    tokio::fs::try_exists(path).await.map_err(|error| {
        TransferIoError::permanent(format!("inspect local artifact {path} failed: {error}"))
    })
}

async fn remote_exists(session: &SftpSessionHandle, path: &str) -> Result<bool, TransferIoError> {
    session.try_exists(path.to_owned()).await.map_err(|error| {
        TransferIoError::from_remote_operation(&format!("inspect remote artifact {path}"), error)
    })
}

async fn rename_artifact(
    connection: &ResolvedSftpConnection,
    job: &TransferJob,
    from: &str,
    to: &str,
) -> Result<(), TransferIoError> {
    match job.direction {
        TransferDirection::Upload => {
            let session = connection.sftp_session().await?;
            session
                .rename(from.to_owned(), to.to_owned())
                .await
                .map_err(|error| {
                    TransferIoError::from_remote_operation("rename remote artifact", error)
                })
        }
        TransferDirection::Download => tokio::fs::rename(from, to).await.map_err(|error| {
            TransferIoError::permanent(format!("rename local artifact failed: {error}"))
        }),
    }
}

async fn rename_artifact_no_replace(
    connection: &ResolvedSftpConnection,
    job: &TransferJob,
    from: &str,
    to: &str,
) -> Result<(), PromotionError> {
    match job.direction {
        TransferDirection::Upload => {
            let session = connection
                .sftp_session()
                .await
                .map_err(PromotionError::Failed)?;
            // russh-sftp sends the base SSH_FXP_RENAME request here. SFTP v3
            // requires that request to fail when `to` already exists; it does
            // not use the overwrite-capable POSIX rename extension.
            match session.rename(from.to_owned(), to.to_owned()).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    let failure = TransferIoError::from_remote_operation(
                        "rename remote artifact without replacement",
                        error,
                    );
                    match remote_exists(&session, to).await {
                        Ok(true) => Err(PromotionError::DestinationExists {
                            message: format!(
                                "destination appeared before non-replacing remote promotion: {to}"
                            ),
                        }),
                        Ok(false) | Err(_) => Err(PromotionError::Failed(failure)),
                    }
                }
            }
        }
        TransferDirection::Download => {
            match tokio::fs::hard_link(from, to).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(PromotionError::DestinationExists {
                        message: format!(
                            "destination appeared before non-replacing local promotion: {to}"
                        ),
                    });
                }
                Err(error) => {
                    return Err(PromotionError::Failed(TransferIoError::permanent(format!(
                        "link local artifact without replacement failed: {error}"
                    ))));
                }
            }
            if let Err(error) = tokio::fs::remove_file(from).await {
                return Err(PromotionError::Ambiguous {
                    message: format!(
                        "linked local artifact to {to}, but could not unlink owned source {from}: {error}; both names may reference the same intact artifact"
                    ),
                });
            }
            Ok(())
        }
    }
}

async fn remove_artifact(
    connection: &ResolvedSftpConnection,
    job: &TransferJob,
    path: &str,
    missing_ok: bool,
) -> Result<(), TransferIoError> {
    match job.direction {
        TransferDirection::Upload => {
            let session = connection.sftp_session().await?;
            if missing_ok && !remote_exists(&session, path).await? {
                return Ok(());
            }
            session.remove_file(path.to_owned()).await.map_err(|error| {
                TransferIoError::from_remote_operation(
                    &format!("delete remote artifact {path}"),
                    error,
                )
            })
        }
        TransferDirection::Download => remove_local_artifact(path, missing_ok).await,
    }
}

async fn remove_local_artifact(path: &str, missing_ok: bool) -> Result<(), TransferIoError> {
    if missing_ok && !local_exists(path).await? {
        return Ok(());
    }
    tokio::fs::remove_file(path).await.map_err(|error| {
        TransferIoError::permanent(format!("delete local artifact {path} failed: {error}"))
    })
}

#[async_trait]
pub trait TransferJobRunner: Send + Sync {
    async fn run(
        &self,
        job: TransferJob,
        control: RunnerControl,
        reporter: RunnerReporter,
        tuning: PipelineTuning,
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

    pub async fn checking(&self) -> Result<(), String> {
        let (ack, response) = oneshot::channel();
        self.send(RunnerEvent::Checking {
            job_id: self.job_id,
            lease_id: self.lease_id,
            ack,
        })?;
        await_ack(response).await
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
        self.commit_phase_with_backup_expectation(phase, None).await
    }

    pub(crate) async fn commit_prepared(&self, backup_expected: bool) -> Result<(), String> {
        self.commit_phase_with_backup_expectation(CommitPhase::Prepared, Some(backup_expected))
            .await
    }

    async fn commit_phase_with_backup_expectation(
        &self,
        phase: CommitPhase,
        backup_expected: Option<bool>,
    ) -> Result<(), String> {
        let (ack, response) = oneshot::channel();
        self.send(RunnerEvent::CommitPhase {
            job_id: self.job_id,
            lease_id: self.lease_id,
            phase,
            backup_expected,
            ack,
        })?;
        await_ack(response).await
    }

    pub fn progress(
        &self,
        bytes: u64,
        speed_bytes_per_second: Option<u64>,
        eta_seconds: Option<u64>,
    ) {
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
    pub(crate) speed_bytes_per_second: Option<u64>,
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
    use std::{
        io::{Cursor, Error, ErrorKind, SeekFrom},
        path::PathBuf,
        pin::Pin,
        sync::{
            Arc, Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll},
    };

    use async_trait::async_trait;
    use parking_lot::Mutex as ParkingMutex;
    use termlab_remote::{
        RemoteError,
        callbacks::{RemoteCallbacks, RemotePaths},
        config::ServerEntry,
        ssh::{SshCredentials, connect_and_auth},
        transfer::{
            ControlDecision, CopyOutcome, SourceFingerprint,
            copy::{CopyError, CopyStage, copy_with_checkpoint_typed},
            fingerprint_open_local, open_sftp_session,
            pipelined::PipelineTuning,
        },
    };
    use tokio::{
        io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncWrite, AsyncWriteExt, ReadBuf},
        sync::{mpsc, watch},
    };
    use uuid::Uuid;

    use super::{
        ConnectionResolver, CopyRangesError, CopyRangesRequest, LiveConnectionIdentity,
        PromotionError, RealPartial, RunnerControl, RunnerControlState, RunnerReporter,
        RunnerResult, SftpTransferJobRunner, TransferIo, TransferIoError, TransferJobRunner,
        cached_resource, classify_pipelined_failure, failed, pipeline_first_window_end,
        recover_commit, remove_local_artifact, select_live_connection_key, sequential_copy_ranges,
    };
    use crate::remote::transfer_queue::{
        artifacts::ArtifactInventory,
        events::RunnerEvent,
        model::{
            AttentionReason, CommitPhase, CompletionResult, ConflictPolicy, ConflictResolution,
            ManagedArtifacts, TransferDirection, TransferEndpoint, TransferJob, TransferJobState,
            TransferOrigin, TransferPriority, TransferProtocol, TransferQueueDocument,
        },
        reducer::{JobEvent, reduce_job},
        scheduler::FailureClass,
        store::TransferStore,
    };
    use crate::remote::{SshConnection, test_remote_state};

    #[derive(Clone, Copy)]
    struct TestConnection;

    #[tokio::test]
    async fn attempt_resource_is_initialized_once_and_new_attempt_gets_fresh_resource() {
        let cell = tokio::sync::OnceCell::new();
        let opens = AtomicUsize::new(0);

        let first = cached_resource(&cell, || async {
            opens.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(String::from("session"))
        })
        .await
        .unwrap();
        let second = cached_resource(&cell, || async {
            opens.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(String::from("unexpected replacement"))
        })
        .await
        .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.as_str(), "session");
        assert_eq!(opens.load(Ordering::SeqCst), 1);

        let next_attempt_cell = tokio::sync::OnceCell::new();
        let next_attempt = cached_resource(&next_attempt_cell, || async {
            opens.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(String::from("fresh session"))
        })
        .await
        .unwrap();

        assert!(!Arc::ptr_eq(&first, &next_attempt));
        assert_eq!(next_attempt.as_str(), "fresh session");
        assert_eq!(opens.load(Ordering::SeqCst), 2);
    }

    struct FailingRead {
        kind: ErrorKind,
        cause: &'static str,
    }

    impl AsyncRead for FailingRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(Error::new(self.kind, self.cause)))
        }
    }

    impl AsyncSeek for FailingRead {
        fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> std::io::Result<()> {
            Ok(())
        }

        fn poll_complete(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(0))
        }
    }

    struct FailingWrite {
        kind: ErrorKind,
        cause: &'static str,
    }

    #[tokio::test]
    async fn failed_local_cleanup_reports_the_exact_leftover_artifact_path() {
        let directory = tempfile::tempdir().unwrap();
        let leftover = directory.path().join("job.termlab-part-exact");
        tokio::fs::create_dir(&leftover).await.unwrap();
        let leftover = leftover.to_string_lossy().into_owned();

        let error = remove_local_artifact(&leftover, false).await.unwrap_err();

        assert!(
            error.message.contains(&leftover),
            "cleanup error must identify the exact owned artifact left behind: {}",
            error.message
        );
    }

    impl AsyncWrite for FailingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(Error::new(self.kind, self.cause)))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncSeek for FailingWrite {
        fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> std::io::Result<()> {
            Ok(())
        }

        fn poll_complete(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(0))
        }
    }

    async fn classify_copy_read_failure(
        direction: TransferDirection,
        kind: ErrorKind,
        cause: &'static str,
    ) -> TransferIoError {
        let mut source = FailingRead { kind, cause };
        let mut destination = Cursor::new(Vec::new());
        let error = copy_with_checkpoint_typed(
            &mut source,
            &mut destination,
            0,
            1,
            1,
            || ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap_err();
        TransferIoError::from_copy(&direction, error)
    }

    async fn classify_copy_write_failure(
        direction: TransferDirection,
        kind: ErrorKind,
        cause: &'static str,
    ) -> TransferIoError {
        let mut source = Cursor::new(b"x".to_vec());
        let mut destination = FailingWrite { kind, cause };
        let error = copy_with_checkpoint_typed(
            &mut source,
            &mut destination,
            0,
            1,
            1,
            || ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap_err();
        TransferIoError::from_copy(&direction, error)
    }

    #[derive(Clone)]
    struct FakeResolver {
        connection: Option<TestConnection>,
        log: Arc<StdMutex<Vec<String>>>,
    }

    impl ConnectionResolver for FakeResolver {
        type Connection = TestConnection;

        fn resolve(&self, _endpoint: &TransferEndpoint) -> Option<Self::Connection> {
            self.log.lock().unwrap().push("resolve".into());
            self.connection
        }
    }

    #[derive(Clone)]
    struct FakeIo {
        state: Arc<StdMutex<FakeIoState>>,
        active_sources: Arc<AtomicUsize>,
    }

    struct FakeIoState {
        log: Arc<StdMutex<Vec<String>>>,
        fingerprint: SourceFingerprint,
        inventory: ArtifactInventory,
        partial_len: Option<u64>,
        final_after_finish_partial: bool,
        final_before_promotion: bool,
        ambiguous_promotion: bool,
        final_identity: Option<&'static str>,
        partial_identity: Option<&'static str>,
        backup_identity: Option<&'static str>,
        fail_at: Option<&'static str>,
        fail_error: Option<TransferIoError>,
        source_bytes: Vec<u8>,
        local_source_path: Option<PathBuf>,
        pending_partial_bytes: Vec<u8>,
        completed_bytes: Vec<u8>,
        opened_fingerprints: Vec<SourceFingerprint>,
    }

    struct TrackedSource {
        cursor: Cursor<Vec<u8>>,
        active_sources: Arc<AtomicUsize>,
    }

    impl AsyncRead for TrackedSource {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.cursor).poll_read(context, buffer)
        }
    }

    impl AsyncSeek for TrackedSource {
        fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
            Pin::new(&mut self.cursor).start_seek(position)
        }

        fn poll_complete(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Pin::new(&mut self.cursor).poll_complete(context)
        }
    }

    impl Drop for TrackedSource {
        fn drop(&mut self) {
            self.active_sources.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl FakeIo {
        fn new(log: Arc<StdMutex<Vec<String>>>, fingerprint: SourceFingerprint) -> Self {
            let source_bytes = vec![b'x'; fingerprint.size as usize];
            Self {
                state: Arc::new(StdMutex::new(FakeIoState {
                    log,
                    fingerprint,
                    inventory: ArtifactInventory {
                        final_exists: false,
                        partial_exists: false,
                        backup_exists: false,
                    },
                    partial_len: None,
                    final_after_finish_partial: false,
                    final_before_promotion: false,
                    ambiguous_promotion: false,
                    final_identity: None,
                    partial_identity: None,
                    backup_identity: None,
                    fail_at: None,
                    fail_error: None,
                    source_bytes,
                    local_source_path: None,
                    pending_partial_bytes: Vec::new(),
                    completed_bytes: Vec::new(),
                    opened_fingerprints: Vec::new(),
                })),
                active_sources: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_local_source(self, path: PathBuf) -> Self {
            self.state.lock().unwrap().local_source_path = Some(path);
            self
        }

        fn replace_remote_source(&self, bytes: &[u8], modified_token: &str) {
            let mut state = self.state.lock().unwrap();
            state.source_bytes = bytes.to_vec();
            state.fingerprint = fingerprint(bytes.len() as u64, Some(modified_token));
        }

        fn completed_bytes(&self) -> Vec<u8> {
            self.state.lock().unwrap().completed_bytes.clone()
        }

        fn opened_fingerprints(&self) -> Vec<SourceFingerprint> {
            self.state.lock().unwrap().opened_fingerprints.clone()
        }

        fn active_source_count(&self) -> usize {
            self.active_sources.load(Ordering::SeqCst)
        }

        fn with_inventory(self, inventory: ArtifactInventory, partial_len: Option<u64>) -> Self {
            {
                let mut state = self.state.lock().unwrap();
                state.inventory = inventory;
                state.partial_len = partial_len;
                state.final_identity = inventory.final_exists.then_some("existing-final");
                state.partial_identity = inventory.partial_exists.then_some("copied-source");
                state.backup_identity = inventory.backup_exists.then_some("owned-backup");
            }
            self
        }

        fn failing_at(self, operation: &'static str) -> Self {
            self.state.lock().unwrap().fail_at = Some(operation);
            self
        }

        fn failing_at_with(self, operation: &'static str, error: TransferIoError) -> Self {
            let mut state = self.state.lock().unwrap();
            state.fail_at = Some(operation);
            state.fail_error = Some(error);
            drop(state);
            self
        }

        fn with_final_appearing_after_copy(self) -> Self {
            self.state.lock().unwrap().final_after_finish_partial = true;
            self
        }

        fn with_final_appearing_before_promotion(self) -> Self {
            self.state.lock().unwrap().final_before_promotion = true;
            self
        }

        fn with_ambiguous_promotion(self) -> Self {
            self.state.lock().unwrap().ambiguous_promotion = true;
            self
        }

        fn artifact_identities(
            &self,
        ) -> (
            Option<&'static str>,
            Option<&'static str>,
            Option<&'static str>,
        ) {
            let state = self.state.lock().unwrap();
            (
                state.final_identity,
                state.partial_identity,
                state.backup_identity,
            )
        }

        fn record(&self, operation: &'static str) -> Result<(), TransferIoError> {
            let mut state = self.state.lock().unwrap();
            state.log.lock().unwrap().push(operation.into());
            if state.fail_at == Some(operation) {
                state.fail_at = None;
                Err(state.fail_error.take().unwrap_or_else(|| {
                    if operation == "open_source" {
                        TransferIoError::transient("connection reset while opening source")
                    } else {
                        TransferIoError::permanent(format!("{operation}: disk full"))
                    }
                }))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl TransferIo<TestConnection> for FakeIo {
        type Source = TrackedSource;
        type Partial = Cursor<Vec<u8>>;

        async fn open_source(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
        ) -> Result<(Self::Source, SourceFingerprint), TransferIoError> {
            self.record("open_source")?;
            let (bytes, fingerprint) = {
                let mut state = self.state.lock().unwrap();
                let (bytes, fingerprint) = if let Some(path) = &state.local_source_path {
                    let bytes = std::fs::read(path).map_err(TransferIoError::from_local)?;
                    let metadata = std::fs::metadata(path).map_err(TransferIoError::from_local)?;
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|value| format!("{}:{}", value.as_nanos(), metadata.len()));
                    let fingerprint = SourceFingerprint {
                        size: metadata.len(),
                        modified_token: modified,
                    };
                    (bytes, fingerprint)
                } else {
                    (state.source_bytes.clone(), state.fingerprint.clone())
                };
                state.opened_fingerprints.push(fingerprint.clone());
                (bytes, fingerprint)
            };
            self.active_sources.fetch_add(1, Ordering::SeqCst);
            Ok((
                TrackedSource {
                    cursor: Cursor::new(bytes),
                    active_sources: Arc::clone(&self.active_sources),
                },
                fingerprint,
            ))
        }

        async fn inventory(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<ArtifactInventory, TransferIoError> {
            self.record("inventory")?;
            Ok(self.state.lock().unwrap().inventory)
        }

        async fn partial_size(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<Option<u64>, TransferIoError> {
            self.record("partial_size")?;
            Ok(self.state.lock().unwrap().partial_len)
        }

        async fn truncate_partial(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
            bytes: u64,
        ) -> Result<(), TransferIoError> {
            self.record("truncate_partial")?;
            self.state.lock().unwrap().partial_len = Some(bytes);
            Ok(())
        }

        async fn open_partial(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
            resume: bool,
        ) -> Result<Self::Partial, TransferIoError> {
            self.record("open_partial")?;
            let mut state = self.state.lock().unwrap();
            let len = if resume {
                state.partial_len.unwrap_or_default()
            } else {
                state.partial_len = Some(0);
                0
            };
            state.inventory.partial_exists = true;
            state.partial_identity = Some("copied-source");
            Ok(Cursor::new(vec![0; len as usize]))
        }

        async fn finish_source(&self, _source: &mut Self::Source) -> Result<(), TransferIoError> {
            self.record("finish_source")
        }

        async fn finish_partial(&self, partial: &mut Self::Partial) -> Result<(), TransferIoError> {
            self.record("finish_partial")?;
            let mut state = self.state.lock().unwrap();
            state.partial_len = Some(partial.get_ref().len() as u64);
            state.pending_partial_bytes = partial.get_ref().clone();
            if state.final_after_finish_partial {
                state.inventory.final_exists = true;
                state.final_identity = Some("late-final");
            }
            Ok(())
        }

        async fn move_final_to_backup(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.record("move_final_to_backup")?;
            let mut state = self.state.lock().unwrap();
            state.inventory.final_exists = false;
            state.inventory.backup_exists = true;
            state.backup_identity = state.final_identity.take();
            Ok(())
        }

        async fn promote_partial_no_replace(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<(), PromotionError> {
            self.record("promote_partial")
                .map_err(PromotionError::Failed)?;
            let mut state = self.state.lock().unwrap();
            if state.final_before_promotion {
                state.inventory.final_exists = true;
                state.final_identity = Some("late-final");
            }
            if state.inventory.final_exists {
                return Err(PromotionError::DestinationExists {
                    message: "destination appeared before fake no-replace promotion".into(),
                });
            }
            if state.ambiguous_promotion {
                state.inventory.final_exists = true;
                state.final_identity = state.partial_identity;
                return Err(PromotionError::Ambiguous {
                    message: "linked final, but could not unlink the owned partial".into(),
                });
            }
            state.inventory.partial_exists = false;
            state.inventory.final_exists = true;
            state.final_identity = state.partial_identity.take();
            state.completed_bytes = std::mem::take(&mut state.pending_partial_bytes);
            state.partial_len = None;
            Ok(())
        }

        async fn restore_backup(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.record("restore_backup")?;
            let mut state = self.state.lock().unwrap();
            state.inventory.backup_exists = false;
            state.inventory.final_exists = true;
            state.final_identity = state.backup_identity.take();
            Ok(())
        }

        async fn delete_backup(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.record("delete_backup")?;
            let mut state = self.state.lock().unwrap();
            state.inventory.backup_exists = false;
            state.backup_identity = None;
            Ok(())
        }

        async fn cleanup_owned_artifacts(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.record("cleanup_owned_artifacts")?;
            assert!(artifacts.partial_path.contains("termlab-part"));
            assert!(artifacts.backup_path.contains("termlab-backup"));
            let mut state = self.state.lock().unwrap();
            state.inventory.partial_exists = false;
            state.inventory.backup_exists = false;
            state.partial_identity = None;
            state.backup_identity = None;
            state.partial_len = None;
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct LocalRecoveryIo {
        open_source_calls: Arc<AtomicUsize>,
    }

    impl LocalRecoveryIo {
        async fn exists(path: &str) -> Result<bool, TransferIoError> {
            tokio::fs::try_exists(path)
                .await
                .map_err(TransferIoError::from_local)
        }

        async fn remove_if_present(path: &str) -> Result<(), TransferIoError> {
            match tokio::fs::remove_file(path).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(TransferIoError::from_local(error)),
            }
        }
    }

    #[async_trait]
    impl TransferIo<()> for LocalRecoveryIo {
        type Source = Cursor<Vec<u8>>;
        type Partial = Cursor<Vec<u8>>;

        async fn open_source(
            &self,
            _connection: &(),
            _job: &TransferJob,
        ) -> Result<(Self::Source, SourceFingerprint), TransferIoError> {
            self.open_source_calls.fetch_add(1, Ordering::SeqCst);
            Err(TransferIoError::permanent(
                "recovery integration unexpectedly reopened the source",
            ))
        }

        async fn inventory(
            &self,
            _connection: &(),
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<ArtifactInventory, TransferIoError> {
            Ok(ArtifactInventory {
                final_exists: Self::exists(&job.local_path).await?,
                partial_exists: Self::exists(&artifacts.partial_path).await?,
                backup_exists: Self::exists(&artifacts.backup_path).await?,
            })
        }

        async fn partial_size(
            &self,
            _connection: &(),
            _job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<Option<u64>, TransferIoError> {
            match tokio::fs::metadata(&artifacts.partial_path).await {
                Ok(metadata) => Ok(Some(metadata.len())),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(error) => Err(TransferIoError::from_local(error)),
            }
        }

        async fn truncate_partial(
            &self,
            _connection: &(),
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
            _bytes: u64,
        ) -> Result<(), TransferIoError> {
            unreachable!("commit recovery must not truncate or recopy")
        }

        async fn open_partial(
            &self,
            _connection: &(),
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
            _resume: bool,
        ) -> Result<Self::Partial, TransferIoError> {
            unreachable!("commit recovery must not open the partial for copying")
        }

        async fn finish_source(&self, _source: &mut Self::Source) -> Result<(), TransferIoError> {
            unreachable!("commit recovery must not open the source")
        }

        async fn finish_partial(
            &self,
            _partial: &mut Self::Partial,
        ) -> Result<(), TransferIoError> {
            unreachable!("commit recovery must not open the partial for copying")
        }

        async fn move_final_to_backup(
            &self,
            _connection: &(),
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            tokio::fs::rename(&job.local_path, &artifacts.backup_path)
                .await
                .map_err(TransferIoError::from_local)
        }

        async fn promote_partial_no_replace(
            &self,
            _connection: &(),
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), PromotionError> {
            if Self::exists(&job.local_path)
                .await
                .map_err(PromotionError::Failed)?
            {
                return Err(PromotionError::DestinationExists {
                    message: "recovery integration destination already exists".into(),
                });
            }
            tokio::fs::rename(&artifacts.partial_path, &job.local_path)
                .await
                .map_err(TransferIoError::from_local)
                .map_err(PromotionError::Failed)
        }

        async fn restore_backup(
            &self,
            _connection: &(),
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            tokio::fs::rename(&artifacts.backup_path, &job.local_path)
                .await
                .map_err(TransferIoError::from_local)
        }

        async fn delete_backup(
            &self,
            _connection: &(),
            _job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            Self::remove_if_present(&artifacts.backup_path).await
        }

        async fn cleanup_owned_artifacts(
            &self,
            _connection: &(),
            _job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            Self::remove_if_present(&artifacts.partial_path).await?;
            Self::remove_if_present(&artifacts.backup_path).await
        }
    }

    fn fingerprint(size: u64, token: Option<&str>) -> SourceFingerprint {
        SourceFingerprint {
            size,
            modified_token: token.map(str::to_owned),
        }
    }

    fn test_job() -> TransferJob {
        let id = Uuid::from_u128(0xaaaa);
        TransferJob {
            id,
            protocol: TransferProtocol::Sftp,
            direction: TransferDirection::Upload,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: "server-a".into(),
                label: "Server A".into(),
            },
            local_path: "/tmp/source.bin".into(),
            remote_path: "/srv/final.bin".into(),
            file_name: "final.bin".into(),
            batch_id: None,
            priority: TransferPriority::Normal,
            queue_order: 1,
            host_key: "configured:server-a".into(),
            destination_key: "configured:server-a:/srv/final.bin".into(),
            state: TransferJobState::Connecting,
            source_fingerprint: None,
            durable_checkpoint: 0,
            bytes_transferred: 0,
            total_bytes: 0,
            speed_bytes_per_second: 0,
            eta_seconds: None,
            retry_attempt: 1,
            max_attempts: 3,
            conflict_policy: ConflictPolicy::Ask,
            artifacts: None,
            commit_phase: CommitPhase::None,
            commit_backup_expected: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            started_at_ms: Some(1),
            finished_at_ms: None,
        }
    }

    #[tokio::test]
    async fn real_partial_observer_reports_only_a_completed_destination_seek() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial.bin");
        tokio::fs::write(&path, b"abcdef").await.unwrap();
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .await
            .unwrap();
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let observer_values = Arc::clone(&observed);
        let mut partial = RealPartial::local_with_test_seek_observer(
            file,
            Arc::new(move |offset| observer_values.lock().unwrap().push(offset)),
        );

        Pin::new(&mut partial)
            .start_seek(SeekFrom::Start(3))
            .unwrap();
        assert!(
            observed.lock().unwrap().is_empty(),
            "a requested seek is not an observed completed seek"
        );
        let completed =
            std::future::poll_fn(|context| Pin::new(&mut partial).poll_complete(context))
                .await
                .unwrap();

        assert_eq!(completed, 3);
        assert_eq!(*observed.lock().unwrap(), vec![3]);
    }

    /// The tuning every legacy `run_fake` test ran under before the pipelined
    /// seam existed: depth 1 (sequential) with the historical 2-byte chunk.
    fn sequential_test_tuning() -> PipelineTuning {
        PipelineTuning {
            depth: 1,
            chunk_bytes: 2,
        }
    }

    /// The live-SSH harness pins the sequential engine at the historical
    /// default chunk size so its seek/progress observations stay comparable.
    fn live_sequential_tuning() -> PipelineTuning {
        PipelineTuning {
            depth: 1,
            chunk_bytes: 256 * 1024,
        }
    }

    async fn run_fake(
        connection: bool,
        io: FakeIo,
        job: TransferJob,
        control_state: super::RunnerControlState,
    ) -> (RunnerResult, Vec<String>) {
        let log = io.state.lock().unwrap().log.clone();
        run_attempt(
            connection,
            io,
            log,
            job,
            control_state,
            sequential_test_tuning(),
        )
        .await
    }

    async fn run_attempt<I>(
        connection: bool,
        io: I,
        log: Arc<StdMutex<Vec<String>>>,
        job: TransferJob,
        control_state: super::RunnerControlState,
        tuning: PipelineTuning,
    ) -> (RunnerResult, Vec<String>)
    where
        I: TransferIo<TestConnection> + 'static,
    {
        let runner = SftpTransferJobRunner::with_services(
            FakeResolver {
                connection: connection.then_some(TestConnection),
                log: log.clone(),
            },
            io,
        );
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let reporter = RunnerReporter::new(job.id, Uuid::from_u128(0xbbbb), event_tx.clone());
        drop(event_tx);
        let event_log = log.clone();
        let acknowledger = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    RunnerEvent::Checking { ack, .. } => {
                        event_log.lock().unwrap().push("checking".into());
                        ack.send(Ok(())).unwrap();
                    }
                    RunnerEvent::Fingerprinted { ack, .. } => {
                        event_log.lock().unwrap().push("fingerprinted".into());
                        ack.send(Ok(())).unwrap();
                    }
                    RunnerEvent::DurableCheckpoint { bytes, ack, .. } => {
                        event_log
                            .lock()
                            .unwrap()
                            .push(format!("checkpoint:{bytes}"));
                        ack.send(Ok(())).unwrap();
                    }
                    RunnerEvent::CommitPhase { phase, ack, .. } => {
                        event_log.lock().unwrap().push(format!("phase:{phase:?}"));
                        ack.send(Ok(())).unwrap();
                    }
                    RunnerEvent::ProgressReady { slot, .. } => {
                        let _ = slot.take_latest_and_release_wake();
                    }
                }
            }
        });
        let (_control_tx, control_rx) = tokio::sync::watch::channel(control_state);

        let result = runner
            .run(job, RunnerControl::new(control_rx), reporter, tuning)
            .await;
        acknowledger.await.unwrap();
        let operations = log.lock().unwrap().clone();
        (result, operations)
    }

    /// A `TransferIo` that delegates every artifact operation to an inner
    /// [`FakeIo`] but overrides `copy_ranges`, so the runner's pipelined seam
    /// (and its one-shot sequential fallback) can be exercised without a live
    /// SFTP connection.
    struct FallbackIo {
        inner: FakeIo,
        depths: Arc<StdMutex<Vec<usize>>>,
        first_failure: StdMutex<Option<CopyRangesError>>,
    }

    impl FallbackIo {
        fn new(inner: FakeIo, first_failure: CopyRangesError) -> Self {
            Self {
                inner,
                depths: Arc::new(StdMutex::new(Vec::new())),
                first_failure: StdMutex::new(Some(first_failure)),
            }
        }
    }

    #[async_trait]
    impl TransferIo<TestConnection> for FallbackIo {
        type Source = <FakeIo as TransferIo<TestConnection>>::Source;
        type Partial = <FakeIo as TransferIo<TestConnection>>::Partial;

        async fn open_source(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
        ) -> Result<(Self::Source, SourceFingerprint), TransferIoError> {
            self.inner.open_source(connection, job).await
        }

        async fn inventory(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<ArtifactInventory, TransferIoError> {
            self.inner.inventory(connection, job, artifacts).await
        }

        async fn partial_size(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<Option<u64>, TransferIoError> {
            self.inner.partial_size(connection, job, artifacts).await
        }

        async fn truncate_partial(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
            bytes: u64,
        ) -> Result<(), TransferIoError> {
            self.inner
                .truncate_partial(connection, job, artifacts, bytes)
                .await
        }

        async fn open_partial(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
            resume: bool,
        ) -> Result<Self::Partial, TransferIoError> {
            self.inner
                .open_partial(connection, job, artifacts, resume)
                .await
        }

        async fn copy_ranges(
            &self,
            _connection: &TestConnection,
            request: CopyRangesRequest<'_>,
            source: &mut Self::Source,
            partial: &mut Self::Partial,
            control: &mut (dyn FnMut() -> ControlDecision + Send),
            progress: &mut (dyn FnMut(u64, u64) + Send),
        ) -> Result<CopyOutcome, CopyRangesError> {
            self.depths.lock().unwrap().push(request.tuning.depth);
            if let Some(failure) = self.first_failure.lock().unwrap().take() {
                return Err(failure);
            }
            sequential_copy_ranges(request, source, partial, control, progress).await
        }

        async fn finish_source(&self, source: &mut Self::Source) -> Result<(), TransferIoError> {
            self.inner.finish_source(source).await
        }

        async fn finish_partial(&self, partial: &mut Self::Partial) -> Result<(), TransferIoError> {
            self.inner.finish_partial(partial).await
        }

        async fn move_final_to_backup(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.inner
                .move_final_to_backup(connection, job, artifacts)
                .await
        }

        async fn promote_partial_no_replace(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), PromotionError> {
            self.inner
                .promote_partial_no_replace(connection, job, artifacts)
                .await
        }

        async fn restore_backup(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.inner.restore_backup(connection, job, artifacts).await
        }

        async fn delete_backup(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.inner.delete_backup(connection, job, artifacts).await
        }

        async fn cleanup_owned_artifacts(
            &self,
            connection: &TestConnection,
            job: &TransferJob,
            artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.inner
                .cleanup_owned_artifacts(connection, job, artifacts)
                .await
        }
    }

    /// Captures `log::warn!` records so the fallback's operator-facing warning
    /// is asserted rather than assumed.
    struct CapturedWarnings;

    static CAPTURED_WARNINGS: StdMutex<Vec<String>> = StdMutex::new(Vec::new());
    static CAPTURE_INSTALLED: std::sync::Once = std::sync::Once::new();

    impl log::Log for CapturedWarnings {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                CAPTURED_WARNINGS.lock().unwrap().push(format!(
                    "{}: {}",
                    record.level(),
                    record.args()
                ));
            }
        }

        fn flush(&self) {}
    }

    fn install_warning_capture() {
        CAPTURE_INSTALLED.call_once(|| {
            if log::set_boxed_logger(Box::new(CapturedWarnings)).is_ok() {
                log::set_max_level(log::LevelFilter::Warn);
            }
        });
    }

    fn captured_warnings_for(job_id: Uuid) -> Vec<String> {
        let needle = job_id.to_string();
        CAPTURED_WARNINGS
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.contains(&needle))
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn a_pipelined_copy_disconnect_keeps_the_sequential_transient_classification() {
        // What `pipelined_copy` produces once `From<CopyError>` flattens it.
        let error = classify_pipelined_failure(RemoteError::Transfer(
            "write destination failed: the SFTP channel closed while writing".into(),
        ));
        let sequential = classify_copy_write_failure(
            TransferDirection::Upload,
            ErrorKind::Other,
            "the SFTP channel closed while writing",
        )
        .await;

        assert_eq!(error.class, FailureClass::Transient);
        assert_eq!(
            error.class, sequential.class,
            "the pipelined engine must not turn a transient disconnect terminal"
        );
    }

    #[test]
    fn a_pipelined_setup_failure_stays_permanent_even_with_a_disconnecting_path() {
        // No copy-stage prefix, so the user-controlled path text is never
        // allowed to fake a transient cause.
        let error = classify_pipelined_failure(RemoteError::Transfer(
            "open local source /tmp/timeout/connection reset.bin for pipelined upload failed: permission denied"
                .into(),
        ));

        assert_eq!(error.class, FailureClass::Permanent);
    }

    #[test]
    fn a_pipelined_copy_permission_failure_stays_permanent() {
        let error = classify_pipelined_failure(RemoteError::Transfer(
            "read source failed: permission denied".into(),
        ));

        assert_eq!(error.class, FailureClass::Permanent);
    }

    #[test]
    fn the_first_window_ends_one_full_window_past_the_resume_offset() {
        let tuning = PipelineTuning {
            depth: 4,
            chunk_bytes: 1024,
        };

        assert_eq!(pipeline_first_window_end(0, 1_000_000, tuning), 4096);
        assert_eq!(pipeline_first_window_end(500, 1_000_000, tuning), 4596);
    }

    #[test]
    fn a_transfer_smaller_than_one_window_is_entirely_first_window() {
        let tuning = PipelineTuning {
            depth: 16,
            chunk_bytes: 262_144,
        };

        assert_eq!(
            pipeline_first_window_end(0, 10, tuning),
            10,
            "progress can never advance past the end of the source"
        );
        assert_eq!(
            pipeline_first_window_end(10, 10, tuning),
            10,
            "a zero-length copy reports no progress at all, so it degrades"
        );
    }

    #[test]
    fn an_absurd_window_saturates_instead_of_overflowing() {
        let tuning = PipelineTuning {
            depth: usize::MAX,
            chunk_bytes: usize::MAX,
        };

        assert_eq!(
            pipeline_first_window_end(u64::MAX - 1, u64::MAX, tuning),
            u64::MAX
        );
    }

    #[tokio::test]
    async fn default_copy_ranges_preserves_sequential_behavior() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log.clone(), fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: false,
                backup_exists: false,
            },
            None,
        );
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;

        // `FakeIo` does not override `copy_ranges`, so a pipelined tuning must
        // still run the sequential default and reproduce the happy path that
        // `overwrite_copies_then_commits_only_after_durable_barriers` asserts.
        let (result, operations) = run_attempt(
            true,
            io,
            log,
            job,
            super::RunnerControlState::Run,
            PipelineTuning {
                depth: 16,
                chunk_bytes: 64,
            },
        )
        .await;

        assert!(matches!(
            result,
            RunnerResult::Completed(super::CompletionResult::Transferred)
        ));
        assert_eq!(
            operations,
            vec![
                "resolve",
                "checking",
                "open_source",
                "fingerprinted",
                "inventory",
                "open_partial",
                "finish_source",
                "finish_partial",
                "checkpoint:4",
                "inventory",
                "phase:Prepared",
                "move_final_to_backup",
                "phase:BackupMoved",
                "promote_partial",
                "phase:PartialPromoted",
                "delete_backup",
                "phase:Complete",
            ]
        );
    }

    #[tokio::test]
    async fn pipelined_failure_in_first_window_falls_back_to_sequential() {
        install_warning_capture();
        let log = Arc::new(StdMutex::new(Vec::new()));
        let inner = FakeIo::new(log.clone(), fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: false,
                backup_exists: false,
            },
            None,
        );
        let io = FallbackIo::new(
            inner,
            CopyRangesError::degrade_to_sequential(
                &TransferDirection::Upload,
                CopyError::Io {
                    stage: CopyStage::WriteDestination,
                    kind: ErrorKind::Other,
                    cause: "concurrency rejected".into(),
                },
            ),
        );
        let depths = Arc::clone(&io.depths);
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;
        let job_id = job.id;

        let (result, operations) = run_attempt(
            true,
            io,
            log,
            job,
            super::RunnerControlState::Run,
            PipelineTuning {
                depth: 16,
                chunk_bytes: 64,
            },
        )
        .await;

        assert_eq!(
            *depths.lock().unwrap(),
            vec![16, 1],
            "a first-window pipelined failure retries exactly once at depth 1"
        );
        assert!(
            matches!(
                result,
                RunnerResult::Completed(super::CompletionResult::Transferred)
            ),
            "the sequential retry completes the attempt, got {result:?}"
        );
        assert!(
            operations.contains(&"phase:Complete".to_string()),
            "the fallback still commits: {operations:?}"
        );
        let warnings = captured_warnings_for(job_id);
        assert_eq!(
            warnings.len(),
            1,
            "exactly one warn-level fallback record: {warnings:?}"
        );
        assert!(
            warnings[0].starts_with("WARN") && warnings[0].contains("concurrency rejected"),
            "the warning names the failure that caused the degrade: {warnings:?}"
        );
    }

    #[tokio::test]
    async fn pipelined_failure_after_first_window_is_a_real_error() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let inner = FakeIo::new(log.clone(), fingerprint(4, Some("v1")));
        let io = FallbackIo::new(
            inner,
            CopyRangesError::from_copy(
                &TransferDirection::Download,
                CopyError::Io {
                    stage: CopyStage::WriteDestination,
                    kind: ErrorKind::PermissionDenied,
                    cause: "write /tmp/timeout/disconnect.bin failed".into(),
                },
            ),
        );
        let depths = Arc::clone(&io.depths);
        let mut job = test_job();
        job.direction = TransferDirection::Download;

        let (result, _operations) = run_attempt(
            true,
            io,
            log,
            job,
            super::RunnerControlState::Run,
            PipelineTuning {
                depth: 16,
                chunk_bytes: 64,
            },
        )
        .await;

        assert_eq!(
            *depths.lock().unwrap(),
            vec![16],
            "a failure past the first window is never retried sequentially"
        );
        let expected = classify_copy_write_failure(
            TransferDirection::Download,
            ErrorKind::PermissionDenied,
            "write /tmp/timeout/disconnect.bin failed",
        )
        .await;
        let RunnerResult::Failed { class, message } = result else {
            panic!("a real pipelined failure must surface as a runner failure, got {result:?}")
        };
        assert_eq!(class, expected.class);
        assert_eq!(message, expected.message);
    }

    struct NoPromptLiveCallbacks;

    #[async_trait]
    impl RemoteCallbacks for NoPromptLiveCallbacks {
        async fn verify_host_key(&self, _message: &str, _fingerprint: &str) -> bool {
            true
        }

        async fn prompt_password(&self, _message: &str) -> Option<String> {
            None
        }

        fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {}
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LiveCommitObservation {
        phase: CommitPhase,
        backup_expected: Option<bool>,
        inventory: ArtifactInventory,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LiveRunObservation {
        destination_seek_offsets: Vec<u64>,
        checkpoints: Vec<u64>,
        progress_bytes: Vec<u64>,
        commits: Vec<LiveCommitObservation>,
    }

    async fn observe_live_artifacts(
        session: &termlab_remote::transfer::SftpSessionHandle,
        job: &TransferJob,
    ) -> Result<ArtifactInventory, String> {
        let artifacts = job
            .artifacts
            .as_ref()
            .ok_or_else(|| "commit phase arrived without managed artifacts".to_string())?;
        match job.direction {
            TransferDirection::Upload => Ok(ArtifactInventory {
                final_exists: session
                    .try_exists(job.remote_path.clone())
                    .await
                    .map_err(|error| format!("inspect live remote final failed: {error}"))?,
                partial_exists: session
                    .try_exists(artifacts.partial_path.clone())
                    .await
                    .map_err(|error| format!("inspect live remote partial failed: {error}"))?,
                backup_exists: session
                    .try_exists(artifacts.backup_path.clone())
                    .await
                    .map_err(|error| format!("inspect live remote backup failed: {error}"))?,
            }),
            TransferDirection::Download => Ok(ArtifactInventory {
                final_exists: tokio::fs::try_exists(&job.local_path)
                    .await
                    .map_err(|error| format!("inspect live local final failed: {error}"))?,
                partial_exists: tokio::fs::try_exists(&artifacts.partial_path)
                    .await
                    .map_err(|error| format!("inspect live local partial failed: {error}"))?,
                backup_exists: tokio::fs::try_exists(&artifacts.backup_path)
                    .await
                    .map_err(|error| format!("inspect live local backup failed: {error}"))?,
            }),
        }
    }

    async fn run_live_transfer(
        runner: &SftpTransferJobRunner,
        job: TransferJob,
        control_state: RunnerControlState,
        session: Arc<termlab_remote::transfer::SftpSessionHandle>,
        destination_seek_offsets: Arc<StdMutex<Vec<u64>>>,
    ) -> Result<(RunnerResult, TransferJob, LiveRunObservation), String> {
        let seek_observation_start = destination_seek_offsets.lock().unwrap().len();
        let observations = Arc::new(StdMutex::new(LiveRunObservation {
            destination_seek_offsets: Vec::new(),
            checkpoints: Vec::new(),
            progress_bytes: Vec::new(),
            commits: Vec::new(),
        }));
        let durable_job = Arc::new(StdMutex::new(job.clone()));
        let event_job = Arc::clone(&durable_job);
        let event_observations = Arc::clone(&observations);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let reporter = RunnerReporter::new(job.id, Uuid::new_v4(), event_tx.clone());
        drop(event_tx);
        let acknowledger =
            tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        RunnerEvent::Checking { ack, .. } => {
                            event_job.lock().unwrap().state = TransferJobState::Checking;
                            let _ = ack.send(Ok(()));
                        }
                        RunnerEvent::Fingerprinted {
                            fingerprint,
                            total_bytes,
                            artifacts,
                            ack,
                            ..
                        } => {
                            let mut job = event_job.lock().unwrap();
                            job.state = TransferJobState::Running;
                            job.source_fingerprint = Some(fingerprint);
                            job.total_bytes = total_bytes;
                            job.artifacts = Some(artifacts);
                            let _ = ack.send(Ok(()));
                        }
                        RunnerEvent::DurableCheckpoint { bytes, ack, .. } => {
                            let mut job = event_job.lock().unwrap();
                            job.durable_checkpoint = bytes;
                            job.bytes_transferred = bytes;
                            event_observations.lock().unwrap().checkpoints.push(bytes);
                            let _ = ack.send(Ok(()));
                        }
                        RunnerEvent::CommitPhase {
                            phase,
                            backup_expected,
                            ack,
                            ..
                        } => {
                            let observed_job = {
                                let mut job = event_job.lock().unwrap();
                                job.commit_phase = phase;
                                if let Some(backup_expected) = backup_expected {
                                    job.commit_backup_expected = Some(backup_expected);
                                }
                                job.clone()
                            };
                            let inventory =
                                match observe_live_artifacts(&session, &observed_job).await {
                                    Ok(inventory) => inventory,
                                    Err(error) => {
                                        let _ = ack.send(Err(error));
                                        continue;
                                    }
                                };
                            event_observations.lock().unwrap().commits.push(
                                LiveCommitObservation {
                                    phase,
                                    backup_expected,
                                    inventory,
                                },
                            );
                            let _ = ack.send(Ok(()));
                        }
                        RunnerEvent::ProgressReady { slot, .. } => {
                            if let Some(progress) = slot.take_latest_and_release_wake() {
                                let mut job = event_job.lock().unwrap();
                                job.bytes_transferred = progress.bytes;
                                job.speed_bytes_per_second =
                                    progress.speed_bytes_per_second.unwrap_or(0);
                                job.eta_seconds = progress.eta_seconds;
                                event_observations
                                    .lock()
                                    .unwrap()
                                    .progress_bytes
                                    .push(progress.bytes);
                            }
                        }
                    }
                }
            });
        let (_control_tx, control_rx) = watch::channel(control_state);
        let result = runner
            .run(
                job,
                RunnerControl::new(control_rx),
                reporter,
                live_sequential_tuning(),
            )
            .await;
        acknowledger
            .await
            .map_err(|error| format!("live transfer event acknowledger failed: {error}"))?;
        let durable_job = durable_job.lock().unwrap().clone();
        observations.lock().unwrap().destination_seek_offsets = destination_seek_offsets
            .lock()
            .unwrap()
            .get(seek_observation_start..)
            .unwrap_or_default()
            .to_vec();
        let observations = observations.lock().unwrap().clone();
        Ok((result, durable_job, observations))
    }

    fn live_job(
        id: Uuid,
        server_id: &str,
        direction: TransferDirection,
        local_path: String,
        remote_path: String,
    ) -> TransferJob {
        let file_name = match direction {
            TransferDirection::Upload => remote_path.rsplit('/').next().unwrap_or("upload.bin"),
            TransferDirection::Download => local_path.rsplit('/').next().unwrap_or("download.bin"),
        }
        .to_owned();
        TransferJob {
            id,
            protocol: TransferProtocol::Sftp,
            direction,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: server_id.to_owned(),
                label: "Disposable OpenSSH".into(),
            },
            local_path,
            remote_path: remote_path.clone(),
            file_name,
            batch_id: None,
            priority: TransferPriority::Normal,
            queue_order: 1,
            host_key: format!("configured:{server_id}"),
            destination_key: format!("configured:{server_id}:{remote_path}"),
            state: TransferJobState::Connecting,
            source_fingerprint: None,
            durable_checkpoint: 0,
            bytes_transferred: 0,
            total_bytes: 0,
            speed_bytes_per_second: 0,
            eta_seconds: None,
            retry_attempt: 1,
            max_attempts: 3,
            conflict_policy: ConflictPolicy::Overwrite,
            artifacts: None,
            commit_phase: CommitPhase::None,
            commit_backup_expected: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            started_at_ms: Some(1),
            finished_at_ms: None,
        }
    }

    async fn write_live_remote(
        session: &termlab_remote::transfer::SftpSessionHandle,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let mut file = session
            .create(path)
            .await
            .map_err(|error| format!("create remote test file {path} failed: {error}"))?;
        file.write_all(bytes)
            .await
            .map_err(|error| format!("write remote test file {path} failed: {error}"))?;
        file.flush()
            .await
            .map_err(|error| format!("flush remote test file {path} failed: {error}"))?;
        file.shutdown()
            .await
            .map_err(|error| format!("close remote test file {path} failed: {error}"))
    }

    async fn read_live_remote(
        session: &termlab_remote::transfer::SftpSessionHandle,
        path: &str,
    ) -> Result<Vec<u8>, String> {
        let mut file = session
            .open(path)
            .await
            .map_err(|error| format!("open remote test file {path} failed: {error}"))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .await
            .map_err(|error| format!("read remote test file {path} failed: {error}"))?;
        file.shutdown()
            .await
            .map_err(|error| format!("close remote test file {path} failed: {error}"))?;
        Ok(bytes)
    }

    async fn cleanup_live_remote_directory(
        session: &termlab_remote::transfer::SftpSessionHandle,
        remote_directory: &str,
    ) -> Result<(), String> {
        if let Ok(entries) = session.read_dir(remote_directory).await {
            for entry in entries {
                let child = format!("{remote_directory}/{}", entry.file_name());
                if entry.metadata().is_dir() {
                    return Err(format!(
                        "refusing recursive cleanup of unexpected directory {child}"
                    ));
                }
                session.remove_file(child.clone()).await.map_err(|error| {
                    format!("remove remote test artifact {child} failed: {error}")
                })?;
            }
        }
        session.remove_dir(remote_directory).await.map_err(|error| {
            format!("remove disposable remote directory {remote_directory} failed: {error}")
        })
    }

    fn require_live(condition: bool, message: impl Into<String>) -> Result<(), String> {
        condition.then_some(()).ok_or_else(|| message.into())
    }

    fn require_live_resume_and_overwrite_trace(
        observation: &LiveRunObservation,
        expected_offset: u64,
        expected_total: u64,
    ) -> Result<(), String> {
        require_live(
            observation.destination_seek_offsets == [expected_offset],
            format!(
                "ordinary runner destination completed seeks {:?} instead of exactly the durable checkpoint {expected_offset}",
                observation.destination_seek_offsets
            ),
        )?;
        require_live(
            observation
                .progress_bytes
                .first()
                .is_some_and(|bytes| *bytes > expected_offset),
            format!(
                "ordinary runner emitted no progress beyond resume offset {expected_offset}: {:?}",
                observation.progress_bytes
            ),
        )?;
        require_live(
            observation.checkpoints.last() == Some(&expected_total),
            format!(
                "ordinary runner did not durably checkpoint total {expected_total}: {:?}",
                observation.checkpoints
            ),
        )?;

        let expected_commits = vec![
            LiveCommitObservation {
                phase: CommitPhase::Prepared,
                backup_expected: Some(true),
                inventory: ArtifactInventory {
                    final_exists: true,
                    partial_exists: true,
                    backup_exists: false,
                },
            },
            LiveCommitObservation {
                phase: CommitPhase::BackupMoved,
                backup_expected: None,
                inventory: ArtifactInventory {
                    final_exists: false,
                    partial_exists: true,
                    backup_exists: true,
                },
            },
            LiveCommitObservation {
                phase: CommitPhase::PartialPromoted,
                backup_expected: None,
                inventory: ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: true,
                },
            },
            LiveCommitObservation {
                phase: CommitPhase::Complete,
                backup_expected: None,
                inventory: ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: false,
                },
            },
        ];
        require_live(
            observation.commits == expected_commits,
            format!(
                "ordinary runner did not observe overwrite backup-to-promotion ordering: {:?}",
                observation.commits
            ),
        )
    }

    #[tokio::test]
    #[ignore = "requires an explicitly configured disposable OpenSSH server"]
    async fn live_sftp_queue_roundtrip() {
        let host = std::env::var("TERMLAB_TEST_SFTP_HOST").ok();
        let port = std::env::var("TERMLAB_TEST_SFTP_PORT").ok();
        let user = std::env::var("TERMLAB_TEST_SFTP_USER").ok();
        let key = std::env::var("TERMLAB_TEST_SFTP_KEY").ok();
        let (Some(host), Some(port), Some(user), Some(key)) = (host, port, user, key) else {
            eprintln!(
                "SKIP live_sftp_queue_roundtrip: set TERMLAB_TEST_SFTP_HOST, \
                 TERMLAB_TEST_SFTP_PORT, TERMLAB_TEST_SFTP_USER, and TERMLAB_TEST_SFTP_KEY \
                 for an explicitly disposable OpenSSH server"
            );
            return;
        };
        let port = port
            .parse::<u16>()
            .expect("TERMLAB_TEST_SFTP_PORT must be a valid u16");
        let local = tempfile::tempdir().expect("create live SFTP test directory");
        let live_id = Uuid::new_v4();
        let server_id = format!("live-sftp-{live_id}");
        let server = ServerEntry {
            id: server_id.clone(),
            label: "Disposable OpenSSH".into(),
            host: host.clone(),
            port,
            user: Some(user.clone()),
            auth_method: Some("key".into()),
            key_path: Some(key.clone()),
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        };
        let credentials = SshCredentials {
            username: user.clone(),
            auth_method: "key".into(),
            password: None,
            key_path: Some(key.clone()),
            key_passphrase: None,
        };
        let paths = RemotePaths {
            known_hosts_file: local.path().join("known_hosts"),
            config_dir: local.path().join("remote-config"),
            default_key_paths: vec![PathBuf::from(&key)],
        };
        let ssh_handle = Arc::new(
            connect_and_auth(
                &server,
                &credentials,
                Arc::new(NoPromptLiveCallbacks),
                &paths,
            )
            .await
            .expect("connect to configured disposable OpenSSH server"),
        );
        let session = Arc::new(
            open_sftp_session(ssh_handle.as_ref())
                .await
                .expect("open disposable SFTP session"),
        );
        let remote_directory = format!("/tmp/{live_id}");
        session
            .create_dir(remote_directory.clone())
            .await
            .expect("create UUID-named disposable remote directory");

        let mut remote_state = test_remote_state();
        remote_state.connections.insert(
            "live-sftp-test".into(),
            SshConnection {
                ssh_handle: Arc::clone(&ssh_handle),
                server_entry_id: Some(server_id.clone()),
                host,
                user,
                port,
                proxy_command: None,
                proxy_jump: None,
                ref_count: 1,
            },
        );
        let destination_seek_offsets = Arc::new(StdMutex::new(Vec::new()));
        let observed_destination_seeks = Arc::clone(&destination_seek_offsets);
        let runner = SftpTransferJobRunner::new_with_partial_seek_observer(
            Arc::new(ParkingMutex::new(remote_state)),
            Arc::new(move |offset| {
                observed_destination_seeks.lock().unwrap().push(offset);
            }),
        );

        let verification = async {
            let upload_path = format!("{remote_directory}/upload.bin");
            let remote_source_path = format!("{remote_directory}/download.bin");
            let local_upload_path = local.path().join("upload.bin");
            let local_download_path = local.path().join("download.bin");
            let upload_bytes: Vec<u8> = (0..700_000).map(|index| (index % 251) as u8).collect();
            let download_bytes: Vec<u8> = (0..710_000)
                .map(|index| (250 - (index % 251)) as u8)
                .collect();
            tokio::fs::write(&local_upload_path, &upload_bytes)
                .await
                .map_err(|error| format!("write local upload fixture failed: {error}"))?;
            write_live_remote(&session, &upload_path, b"old remote destination").await?;

            let upload = live_job(
                Uuid::new_v4(),
                &server_id,
                TransferDirection::Upload,
                local_upload_path.to_string_lossy().into_owned(),
                upload_path.clone(),
            );
            let (paused_result, mut upload, _) = run_live_transfer(
                &runner,
                upload,
                RunnerControlState::Pause,
                Arc::clone(&session),
                Arc::clone(&destination_seek_offsets),
            )
            .await?;
            let RunnerResult::Paused { durable_checkpoint } = paused_result else {
                return Err(format!("upload did not pause: {paused_result:?}"));
            };
            require_live(
                durable_checkpoint > 0 && durable_checkpoint < upload_bytes.len() as u64,
                format!("upload pause checkpoint was not resumable: {durable_checkpoint}"),
            )?;
            let upload_artifacts = upload
                .artifacts
                .clone()
                .ok_or_else(|| "paused upload did not persist managed artifacts".to_string())?;
            require_live(
                read_live_remote(&session, &upload_artifacts.partial_path)
                    .await?
                    .len() as u64
                    == durable_checkpoint,
                "remote upload partial length did not match the durable checkpoint",
            )?;
            require_live(
                read_live_remote(&session, &upload_path).await? == b"old remote destination",
                "paused overwrite changed the existing remote destination before commit",
            )?;

            upload.state = TransferJobState::Connecting;
            let upload_resume_checkpoint = durable_checkpoint;
            let (upload_result, uploaded, upload_observation) = run_live_transfer(
                &runner,
                upload,
                RunnerControlState::Run,
                Arc::clone(&session),
                Arc::clone(&destination_seek_offsets),
            )
            .await?;
            require_live(
                matches!(
                    upload_result,
                    RunnerResult::Completed(CompletionResult::Transferred)
                ),
                format!("resumed upload did not complete: {upload_result:?}"),
            )?;
            require_live_resume_and_overwrite_trace(
                &upload_observation,
                upload_resume_checkpoint,
                upload_bytes.len() as u64,
            )?;
            require_live(
                read_live_remote(&session, &upload_path).await? == upload_bytes,
                "resumed upload did not promote the complete new bytes",
            )?;
            require_live(
                session
                    .metadata(upload_path.clone())
                    .await
                    .map_err(|error| format!("stat uploaded file failed: {error}"))?
                    .mtime
                    .is_some(),
                "OpenSSH did not expose a remote mtime for the uploaded file",
            )?;
            require_live(
                !session
                    .try_exists(upload_artifacts.partial_path.clone())
                    .await
                    .map_err(|error| format!("inspect upload partial cleanup failed: {error}"))?
                    && !session
                        .try_exists(upload_artifacts.backup_path.clone())
                        .await
                        .map_err(|error| {
                            format!("inspect upload backup cleanup failed: {error}")
                        })?,
                "completed upload left a managed partial or backup",
            )?;
            require_live(
                uploaded
                    .source_fingerprint
                    .as_ref()
                    .and_then(|fingerprint| fingerprint.modified_token.as_deref())
                    .is_some_and(|token| token.starts_with("unixNs:")),
                "upload did not retain its local nanosecond mtime fingerprint",
            )?;

            write_live_remote(&session, &remote_source_path, &download_bytes).await?;
            tokio::fs::write(&local_download_path, b"old local destination")
                .await
                .map_err(|error| format!("write local download destination failed: {error}"))?;
            let download = live_job(
                Uuid::new_v4(),
                &server_id,
                TransferDirection::Download,
                local_download_path.to_string_lossy().into_owned(),
                remote_source_path.clone(),
            );
            let (paused_result, mut download, _) = run_live_transfer(
                &runner,
                download,
                RunnerControlState::Pause,
                Arc::clone(&session),
                Arc::clone(&destination_seek_offsets),
            )
            .await?;
            let RunnerResult::Paused { durable_checkpoint } = paused_result else {
                return Err(format!("download did not pause: {paused_result:?}"));
            };
            require_live(
                durable_checkpoint > 0 && durable_checkpoint < download_bytes.len() as u64,
                format!("download pause checkpoint was not resumable: {durable_checkpoint}"),
            )?;
            require_live(
                download
                    .source_fingerprint
                    .as_ref()
                    .and_then(|fingerprint| fingerprint.modified_token.as_deref())
                    .is_some_and(|token| token.starts_with("unixSeconds:")),
                "download did not persist the remote second-resolution mtime fingerprint",
            )?;
            let download_artifacts = download
                .artifacts
                .clone()
                .ok_or_else(|| "paused download did not persist managed artifacts".to_string())?;
            require_live(
                tokio::fs::metadata(&download_artifacts.partial_path)
                    .await
                    .map_err(|error| format!("stat local download partial failed: {error}"))?
                    .len()
                    == durable_checkpoint,
                "local download partial length did not match the durable checkpoint",
            )?;
            require_live(
                tokio::fs::read(&local_download_path)
                    .await
                    .map_err(|error| format!("read paused local destination failed: {error}"))?
                    == b"old local destination",
                "paused overwrite changed the existing local destination before commit",
            )?;

            download.state = TransferJobState::Connecting;
            let download_resume_checkpoint = durable_checkpoint;
            let (download_result, _, download_observation) = run_live_transfer(
                &runner,
                download,
                RunnerControlState::Run,
                Arc::clone(&session),
                Arc::clone(&destination_seek_offsets),
            )
            .await?;
            require_live(
                matches!(
                    download_result,
                    RunnerResult::Completed(CompletionResult::Transferred)
                ),
                format!("resumed download did not complete: {download_result:?}"),
            )?;
            require_live_resume_and_overwrite_trace(
                &download_observation,
                download_resume_checkpoint,
                download_bytes.len() as u64,
            )?;
            require_live(
                tokio::fs::read(&local_download_path)
                    .await
                    .map_err(|error| format!("read completed local destination failed: {error}"))?
                    == download_bytes,
                "resumed download did not promote the complete new bytes",
            )?;
            require_live(
                !tokio::fs::try_exists(&download_artifacts.partial_path)
                    .await
                    .map_err(|error| format!("inspect download partial cleanup failed: {error}"))?
                    && !tokio::fs::try_exists(&download_artifacts.backup_path)
                        .await
                        .map_err(|error| {
                            format!("inspect download backup cleanup failed: {error}")
                        })?,
                "completed download left a managed partial or backup",
            )?;

            let latest_upload = b"latest upload bytes from the second transfer".to_vec();
            tokio::fs::write(&local_upload_path, &latest_upload)
                .await
                .map_err(|error| format!("rewrite local upload source failed: {error}"))?;
            let second_upload = live_job(
                Uuid::new_v4(),
                &server_id,
                TransferDirection::Upload,
                local_upload_path.to_string_lossy().into_owned(),
                upload_path.clone(),
            );
            let (second_upload_result, second_upload, _) = run_live_transfer(
                &runner,
                second_upload,
                RunnerControlState::Run,
                Arc::clone(&session),
                Arc::clone(&destination_seek_offsets),
            )
            .await?;
            require_live(
                matches!(
                    second_upload_result,
                    RunnerResult::Completed(CompletionResult::Transferred)
                ) && second_upload
                    .source_fingerprint
                    .as_ref()
                    .is_some_and(|fingerprint| fingerprint.size == latest_upload.len() as u64)
                    && read_live_remote(&session, &upload_path).await? == latest_upload,
                "second upload reused stale source bytes or fingerprint",
            )?;

            let latest_download = b"latest remote bytes from the second download".to_vec();
            write_live_remote(&session, &remote_source_path, &latest_download).await?;
            let second_download = live_job(
                Uuid::new_v4(),
                &server_id,
                TransferDirection::Download,
                local_download_path.to_string_lossy().into_owned(),
                remote_source_path.clone(),
            );
            let (second_download_result, second_download, _) = run_live_transfer(
                &runner,
                second_download,
                RunnerControlState::Run,
                Arc::clone(&session),
                Arc::clone(&destination_seek_offsets),
            )
            .await?;
            require_live(
                matches!(
                    second_download_result,
                    RunnerResult::Completed(CompletionResult::Transferred)
                ) && second_download
                    .source_fingerprint
                    .as_ref()
                    .is_some_and(|fingerprint| fingerprint.size == latest_download.len() as u64)
                    && tokio::fs::read(&local_download_path)
                        .await
                        .map_err(|error| format!("read second downloaded file failed: {error}"))?
                        == latest_download,
                "second download reused stale source bytes or fingerprint",
            )?;

            Ok::<(), String>(())
        }
        .await;

        let cleanup = cleanup_live_remote_directory(&session, &remote_directory).await;
        if let Err(error) = verification {
            panic!("live SFTP queue verification failed: {error}; cleanup result: {cleanup:?}");
        }
        cleanup.expect("clean only the exact UUID-named disposable remote directory");
    }

    #[tokio::test]
    async fn missing_live_connection_returns_before_any_transfer_io() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log.clone(), fingerprint(4, Some("v1")));

        let (result, operations) =
            run_fake(false, io, test_job(), super::RunnerControlState::Run).await;

        assert!(matches!(result, RunnerResult::NeedsConnection(_)));
        assert_eq!(operations, vec!["resolve"]);
    }

    #[tokio::test]
    async fn overwrite_copies_then_commits_only_after_durable_barriers() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: false,
                backup_exists: false,
            },
            None,
        );
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Completed(super::CompletionResult::Transferred)
        ));
        assert_eq!(
            operations,
            vec![
                "resolve",
                "checking",
                "open_source",
                "fingerprinted",
                "inventory",
                "open_partial",
                "finish_source",
                "finish_partial",
                "checkpoint:4",
                "inventory",
                "phase:Prepared",
                "move_final_to_backup",
                "phase:BackupMoved",
                "promote_partial",
                "phase:PartialPromoted",
                "delete_backup",
                "phase:Complete",
            ]
        );
    }

    #[tokio::test]
    async fn prepared_commit_recovers_without_reopening_or_recopying_the_source() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: true,
                backup_exists: false,
            },
            Some(4),
        );
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 4;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        job.commit_phase = CommitPhase::Prepared;
        job.commit_backup_expected = Some(true);

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Completed(super::CompletionResult::Transferred)
        ));
        assert_eq!(
            operations,
            vec![
                "resolve",
                "checking",
                "inventory",
                "move_final_to_backup",
                "phase:BackupMoved",
                "inventory",
                "promote_partial",
                "phase:PartialPromoted",
                "inventory",
                "delete_backup",
                "phase:Complete",
            ]
        );
    }

    #[tokio::test]
    async fn backup_moved_recovery_defers_to_the_authoritative_artifact_policy() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: false,
                backup_exists: false,
            },
            None,
        );
        let inspected_io = io.clone();
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 4;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        job.commit_phase = CommitPhase::BackupMoved;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        let RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { message }) = result
        else {
            panic!("an unowned final without the persisted backup must not be assumed promoted")
        };
        assert!(message.contains("BackupMoved"));
        assert_eq!(
            inspected_io.artifact_identities(),
            (Some("existing-final"), None, None)
        );
        assert!(!operations.iter().any(|entry| entry == "phase:Complete"));
    }

    #[tokio::test]
    async fn restart_after_each_proven_commit_phase_completes_without_recopying() {
        let cases = [
            (
                CommitPhase::Prepared,
                ArtifactInventory {
                    final_exists: true,
                    partial_exists: true,
                    backup_exists: false,
                },
            ),
            (
                CommitPhase::BackupMoved,
                ArtifactInventory {
                    final_exists: false,
                    partial_exists: true,
                    backup_exists: true,
                },
            ),
            (
                CommitPhase::PartialPromoted,
                ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: true,
                },
            ),
            (
                CommitPhase::CleanupPending,
                ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: true,
                },
            ),
            (
                CommitPhase::Complete,
                ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: false,
                },
            ),
        ];

        for (phase, inventory) in cases {
            let log = Arc::new(StdMutex::new(Vec::new()));
            let io = FakeIo::new(log, fingerprint(4, Some("v1")))
                .with_inventory(inventory, inventory.partial_exists.then_some(4));
            let mut job = test_job();
            job.conflict_policy = ConflictPolicy::Overwrite;
            job.source_fingerprint = Some(fingerprint(4, Some("v1")));
            job.durable_checkpoint = 4;
            job.artifacts =
                Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
            job.commit_phase = phase;
            job.commit_backup_expected = (phase != CommitPhase::Complete).then_some(true);

            let (result, operations) =
                run_fake(true, io, job, super::RunnerControlState::Run).await;

            assert!(
                matches!(
                    result,
                    RunnerResult::Completed(CompletionResult::Transferred)
                ),
                "phase {phase:?}, inventory {inventory:?}, operations {operations:?}, result {result:?}"
            );
            assert!(
                !operations
                    .iter()
                    .any(|operation| operation == "open_source")
            );
            assert!(
                !operations
                    .iter()
                    .any(|operation| operation == "open_partial")
            );
        }
    }

    #[tokio::test]
    async fn restarted_store_recovers_real_fresh_and_overwrite_artifacts_without_recopying() {
        struct Case {
            name: &'static str,
            phase: CommitPhase,
            backup_expected: bool,
            final_bytes: Option<&'static [u8]>,
            partial_bytes: Option<&'static [u8]>,
            backup_bytes: Option<&'static [u8]>,
        }

        let cases = [
            Case {
                name: "fresh-prepared-before-promotion",
                phase: CommitPhase::Prepared,
                backup_expected: false,
                final_bytes: None,
                partial_bytes: Some(b"new"),
                backup_bytes: None,
            },
            Case {
                name: "fresh-backup-moved-before-promotion",
                phase: CommitPhase::BackupMoved,
                backup_expected: false,
                final_bytes: None,
                partial_bytes: Some(b"new"),
                backup_bytes: None,
            },
            Case {
                name: "fresh-backup-moved-after-promotion-before-phase-write",
                phase: CommitPhase::BackupMoved,
                backup_expected: false,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: None,
            },
            Case {
                name: "fresh-partial-promoted",
                phase: CommitPhase::PartialPromoted,
                backup_expected: false,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: None,
            },
            Case {
                name: "fresh-cleanup-pending",
                phase: CommitPhase::CleanupPending,
                backup_expected: false,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: None,
            },
            Case {
                name: "fresh-complete",
                phase: CommitPhase::Complete,
                backup_expected: false,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: None,
            },
            Case {
                name: "overwrite-prepared-before-backup",
                phase: CommitPhase::Prepared,
                backup_expected: true,
                final_bytes: Some(b"old"),
                partial_bytes: Some(b"new"),
                backup_bytes: None,
            },
            Case {
                name: "overwrite-prepared-after-backup-before-phase-write",
                phase: CommitPhase::Prepared,
                backup_expected: true,
                final_bytes: None,
                partial_bytes: Some(b"new"),
                backup_bytes: Some(b"old"),
            },
            Case {
                name: "overwrite-backup-moved-after-promotion-before-phase-write",
                phase: CommitPhase::BackupMoved,
                backup_expected: true,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: Some(b"old"),
            },
            Case {
                name: "overwrite-backup-moved",
                phase: CommitPhase::BackupMoved,
                backup_expected: true,
                final_bytes: None,
                partial_bytes: Some(b"new"),
                backup_bytes: Some(b"old"),
            },
            Case {
                name: "overwrite-partial-promoted",
                phase: CommitPhase::PartialPromoted,
                backup_expected: true,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: Some(b"old"),
            },
            Case {
                name: "overwrite-cleanup-pending",
                phase: CommitPhase::CleanupPending,
                backup_expected: true,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: Some(b"old"),
            },
            Case {
                name: "overwrite-complete",
                phase: CommitPhase::Complete,
                backup_expected: true,
                final_bytes: Some(b"new"),
                partial_bytes: None,
                backup_bytes: None,
            },
        ];

        for case in cases {
            let directory = tempfile::tempdir().unwrap();
            let store_path = directory.path().join("transfer-queue.json");
            let final_path = directory.path().join("final.bin");
            let mut job = test_job();
            job.direction = TransferDirection::Download;
            job.conflict_policy = ConflictPolicy::Overwrite;
            job.local_path = final_path.to_string_lossy().into_owned();
            job.remote_path = "/source.bin".into();
            job.destination_key = format!("configured:server-a:{}", job.local_path);
            job.state = TransferJobState::Paused;
            job.source_fingerprint = Some(fingerprint(3, Some("source-v1")));
            job.durable_checkpoint = 3;
            job.bytes_transferred = 3;
            job.total_bytes = 3;
            let artifacts =
                ManagedArtifacts::for_local_destination(job.id, final_path.as_path()).unwrap();
            job.artifacts = Some(artifacts.clone());
            job.commit_phase = case.phase;
            job.commit_backup_expected = Some(case.backup_expected);

            for (path, bytes) in [
                (job.local_path.as_str(), case.final_bytes),
                (artifacts.partial_path.as_str(), case.partial_bytes),
                (artifacts.backup_path.as_str(), case.backup_bytes),
            ] {
                if let Some(bytes) = bytes {
                    tokio::fs::write(path, bytes).await.unwrap();
                }
            }

            let store = TransferStore::new(store_path.clone());
            store
                .save(&TransferQueueDocument {
                    jobs: vec![job],
                    ..TransferQueueDocument::default()
                })
                .unwrap();
            drop(store);

            // This load is the process-restart boundary: recovery receives the
            // actual persisted v1 job, not the in-memory value saved above.
            let restarted = TransferStore::new(store_path.clone());
            let document = restarted.load().unwrap().into_document();
            let restarted_job = document.jobs[0].clone();
            drop(restarted);

            let (event_tx, mut event_rx) = mpsc::unbounded_channel();
            let reporter = RunnerReporter::new(restarted_job.id, Uuid::new_v4(), event_tx.clone());
            drop(event_tx);
            let persisted_path = store_path.clone();
            let acknowledger = tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        RunnerEvent::CommitPhase {
                            phase,
                            backup_expected,
                            ack,
                            ..
                        } => {
                            let result = (|| -> Result<(), String> {
                                let store = TransferStore::new(persisted_path.clone());
                                let mut document = store
                                    .load()
                                    .map_err(|error| error.to_string())?
                                    .into_document();
                                let persisted_job = document
                                    .jobs
                                    .first_mut()
                                    .ok_or_else(|| "restarted job disappeared".to_string())?;
                                persisted_job.commit_phase = phase;
                                if let Some(backup_expected) = backup_expected {
                                    persisted_job.commit_backup_expected = Some(backup_expected);
                                }
                                store.save(&document).map_err(|error| error.to_string())
                            })();
                            let _ = ack.send(result);
                        }
                        other => panic!("unexpected recovery event: {other:?}"),
                    }
                }
            });

            let io = LocalRecoveryIo::default();
            let result = recover_commit(&io, &(), &restarted_job, &artifacts, &reporter).await;
            drop(reporter);
            acknowledger.await.unwrap();

            assert!(
                matches!(
                    result,
                    RunnerResult::Completed(CompletionResult::Transferred)
                ),
                "case {} returned {result:?}",
                case.name
            );
            assert_eq!(
                io.open_source_calls.load(Ordering::SeqCst),
                0,
                "case {} recopied the source",
                case.name
            );
            assert_eq!(
                tokio::fs::read(&final_path).await.unwrap(),
                b"new",
                "case {} did not preserve the promoted bytes",
                case.name
            );
            assert!(
                !tokio::fs::try_exists(&artifacts.partial_path)
                    .await
                    .unwrap(),
                "case {} retained its partial",
                case.name
            );
            assert!(
                !tokio::fs::try_exists(&artifacts.backup_path).await.unwrap(),
                "case {} retained its backup",
                case.name
            );
            let completed = TransferStore::new(store_path)
                .load()
                .unwrap()
                .into_document();
            assert_eq!(
                completed.jobs[0].commit_phase,
                CommitPhase::Complete,
                "case {} did not persist completion",
                case.name
            );
            assert_eq!(
                completed.jobs[0].commit_backup_expected,
                Some(case.backup_expected),
                "case {} lost commit provenance",
                case.name
            );
        }
    }

    #[tokio::test]
    async fn prepared_ask_recovery_preserves_a_late_final_and_owned_partial() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: true,
                backup_exists: false,
            },
            Some(4),
        );
        let inspected_io = io.clone();
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 4;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        job.commit_phase = CommitPhase::Prepared;
        job.commit_backup_expected = Some(false);

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { .. })
        ));
        assert_eq!(
            inspected_io.artifact_identities(),
            (Some("existing-final"), Some("copied-source"), None)
        );
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "move_final_to_backup")
        );
        assert!(!operations.iter().any(|entry| entry == "promote_partial"));
    }

    #[tokio::test]
    async fn resume_authorization_moves_one_late_final_then_recovers_backup_moved_commit() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: true,
                backup_exists: false,
            },
            Some(4),
        );
        let inspected_io = io.clone();
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 4;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        job.commit_phase = CommitPhase::BackupMoved;
        job.commit_backup_expected = Some(false);
        job.conflict_policy = ConflictPolicy::Overwrite;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Completed(super::CompletionResult::Transferred)
        ));
        assert_eq!(
            inspected_io.artifact_identities(),
            (Some("copied-source"), None, None)
        );
        let moved = operations
            .iter()
            .position(|entry| entry == "move_final_to_backup")
            .unwrap();
        let acknowledged = operations
            .iter()
            .position(|entry| entry == "phase:BackupMoved")
            .unwrap();
        let promoted = operations
            .iter()
            .position(|entry| entry == "promote_partial")
            .unwrap();
        assert!(moved < acknowledged && acknowledged < promoted);
    }

    #[tokio::test]
    async fn overwrite_recovery_never_recreates_a_missing_authoritative_backup() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: true,
                backup_exists: false,
            },
            Some(4),
        );
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 4;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        job.commit_phase = CommitPhase::BackupMoved;
        job.commit_backup_expected = Some(true);
        job.conflict_policy = ConflictPolicy::Overwrite;

        let (result, operations) = run_fake(true, io, job, RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { .. })
        ));
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "move_final_to_backup")
        );
        assert!(!operations.iter().any(|entry| entry == "promote_partial"));
    }

    #[tokio::test]
    async fn promotion_failure_restores_backup_and_reports_exact_remaining_inventory() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1")))
            .with_inventory(
                ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: false,
                },
                None,
            )
            .failing_at("promote_partial");
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        let RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { message }) = result
        else {
            panic!("promotion failure must require conservative commit recovery")
        };
        assert!(
            message.contains("final=true, partial=true, backup=false"),
            "unexpected recovery inventory: {message}"
        );
        let promote = operations
            .iter()
            .position(|entry| entry == "promote_partial")
            .unwrap();
        let restore = operations
            .iter()
            .position(|entry| entry == "restore_backup")
            .unwrap();
        let inventory = operations
            .iter()
            .rposition(|entry| entry == "inventory")
            .unwrap();
        assert!(promote < restore && restore < inventory);
        assert!(!operations.iter().any(|entry| entry == "delete_backup"));
        assert!(!operations.iter().any(|entry| entry == "phase:Complete"));
    }

    #[tokio::test]
    async fn fresh_copy_without_modified_token_is_allowed() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, None));
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;

        let (result, _) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Completed(super::CompletionResult::Transferred)
        ));
    }

    #[tokio::test]
    async fn durable_checkpoint_without_its_owned_partial_requires_attention() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1")));
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 2;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::MissingPartial)
        ));
        assert!(!operations.iter().any(|entry| entry == "open_partial"));
    }

    #[tokio::test]
    async fn partial_that_disappears_during_reconciliation_requires_attention() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            None,
        );
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 2;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::MissingPartial)
        ));
        assert!(!operations.iter().any(|entry| entry == "open_partial"));
    }

    #[tokio::test]
    async fn incompatible_persisted_artifacts_enter_checking_before_attention() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1")));
        let mut job = test_job();
        job.artifacts = Some(ManagedArtifacts {
            partial_path: "/srv/not-this-jobs-partial".into(),
            backup_path: "/srv/not-this-jobs-backup".into(),
        });

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { .. })
        ));
        assert_eq!(operations, vec!["resolve", "checking"]);
    }

    #[tokio::test]
    async fn changed_source_is_rejected_before_opening_or_mutating_the_partial() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v2")));
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 2;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::SourceChanged { .. })
        ));
        assert_eq!(
            operations,
            vec!["resolve", "checking", "open_source", "finish_source"]
        );
    }

    #[tokio::test]
    async fn a_second_upload_reopens_the_local_path_and_uses_its_latest_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("source.txt");
        std::fs::write(&source_path, b"old").unwrap();
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(0, None)).with_local_source(source_path.clone());
        let inspected_io = io.clone();
        let mut first = test_job();
        first.local_path = source_path.to_string_lossy().into_owned();
        first.conflict_policy = ConflictPolicy::Overwrite;

        let (first_result, _) =
            run_fake(true, io.clone(), first, super::RunnerControlState::Run).await;

        assert!(matches!(first_result, RunnerResult::Completed(_)));
        assert_eq!(inspected_io.completed_bytes(), b"old");
        assert_eq!(inspected_io.active_source_count(), 0);

        std::fs::write(&source_path, b"new version").unwrap();
        let mut second = test_job();
        second.id = Uuid::from_u128(0xaaab);
        second.local_path = source_path.to_string_lossy().into_owned();
        second.conflict_policy = ConflictPolicy::Overwrite;
        let (second_result, _) = run_fake(true, io, second, super::RunnerControlState::Run).await;

        assert!(matches!(second_result, RunnerResult::Completed(_)));
        assert_eq!(inspected_io.completed_bytes(), b"new version");
        let fingerprints = inspected_io.opened_fingerprints();
        assert_eq!(fingerprints.len(), 2);
        assert_ne!(fingerprints[0], fingerprints[1]);
        assert_eq!(fingerprints[1].size, 11);
        assert_eq!(inspected_io.active_source_count(), 0);
    }

    #[tokio::test]
    async fn a_second_download_reopens_the_remote_path_and_uses_its_latest_bytes() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(3, Some("remote-v1")));
        io.replace_remote_source(b"old", "remote-v1");
        let inspected_io = io.clone();
        let mut first = test_job();
        first.direction = TransferDirection::Download;
        first.local_path = "/tmp/downloaded.txt".into();
        first.conflict_policy = ConflictPolicy::Overwrite;

        let (first_result, _) =
            run_fake(true, io.clone(), first, super::RunnerControlState::Run).await;

        assert!(matches!(first_result, RunnerResult::Completed(_)));
        assert_eq!(inspected_io.completed_bytes(), b"old");
        assert_eq!(inspected_io.active_source_count(), 0);

        inspected_io.replace_remote_source(b"new remote version", "remote-v2");
        let mut second = test_job();
        second.id = Uuid::from_u128(0xaaac);
        second.direction = TransferDirection::Download;
        second.local_path = "/tmp/downloaded.txt".into();
        second.conflict_policy = ConflictPolicy::Overwrite;
        let (second_result, _) = run_fake(true, io, second, super::RunnerControlState::Run).await;

        assert!(matches!(second_result, RunnerResult::Completed(_)));
        assert_eq!(inspected_io.completed_bytes(), b"new remote version");
        assert_eq!(
            inspected_io.opened_fingerprints(),
            vec![
                fingerprint(3, Some("remote-v1")),
                fingerprint(18, Some("remote-v2")),
            ]
        );
        assert_eq!(inspected_io.active_source_count(), 0);
    }

    #[tokio::test]
    async fn paused_source_change_requires_attention_until_restart_clears_the_attempt() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v2"))).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        io.replace_remote_source(b"bbbb", "v2");
        let inspected_io = io.clone();
        let mut paused = test_job();
        paused.state = TransferJobState::Paused;
        paused.source_fingerprint = Some(fingerprint(4, Some("v1")));
        paused.durable_checkpoint = 2;
        paused.artifacts =
            Some(ManagedArtifacts::for_destination(paused.id, &paused.remote_path).unwrap());
        paused.conflict_policy = ConflictPolicy::Overwrite;

        let (result, _) = run_fake(
            true,
            io.clone(),
            paused.clone(),
            super::RunnerControlState::Run,
        )
        .await;

        let RunnerResult::NeedsAttention(reason @ AttentionReason::SourceChanged { .. }) = result
        else {
            panic!("paused source mutation must require an explicit restart")
        };
        assert_eq!(inspected_io.active_source_count(), 0);

        paused.state = TransferJobState::NeedsAttention { reason };
        let restarted = reduce_job(
            &paused,
            JobEvent::ResolveAfterCleanup(ConflictResolution::Restart),
            2,
        )
        .unwrap();
        assert_eq!(restarted.source_fingerprint, None);
        assert_eq!(restarted.durable_checkpoint, 0);
        assert_eq!(restarted.artifacts, None);

        let (result, operations) =
            run_fake(true, io, restarted, super::RunnerControlState::Run).await;

        assert!(matches!(result, RunnerResult::Completed(_)));
        assert_eq!(inspected_io.completed_bytes(), b"bbbb");
        assert!(
            operations
                .iter()
                .any(|operation| operation == "truncate_partial")
        );
        assert_eq!(inspected_io.active_source_count(), 0);
    }

    #[tokio::test]
    async fn unverifiable_interrupted_source_cannot_resume_even_when_size_matches() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, None)).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, None));
        job.durable_checkpoint = 2;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::SourceCannotResume)
        ));
        assert_eq!(operations, vec!["resolve", "checking"]);
    }

    #[tokio::test]
    async fn checkpointed_resume_without_a_persisted_fingerprint_is_rejected_before_partial_io() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        let mut job = test_job();
        job.durable_checkpoint = 2;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::SourceCannotResume)
        ));
        assert_eq!(operations, vec!["resolve", "checking"]);
    }

    #[tokio::test]
    async fn checkpointed_resume_without_persisted_owned_artifacts_is_rejected_before_partial_io() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 2;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::SourceCannotResume)
        ));
        assert_eq!(operations, vec!["resolve", "checking"]);
    }

    #[tokio::test]
    async fn checkpointed_resume_with_mismatched_owned_artifacts_is_rejected_before_partial_io() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(4, Some("v1")));
        job.durable_checkpoint = 2;
        job.artifacts = Some(ManagedArtifacts {
            partial_path: "/srv/not-this-jobs-partial".into(),
            backup_path: "/srv/not-this-jobs-backup".into(),
        });

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::SourceCannotResume)
        ));
        assert_eq!(operations, vec!["resolve", "checking"]);
    }

    #[tokio::test]
    async fn tokenless_attention_restart_recreates_owned_partial_as_a_fresh_copy() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, None)).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        let mut job = test_job();
        job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::SourceCannotResume,
        };
        job.source_fingerprint = Some(fingerprint(4, None));
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        let job = reduce_job(
            &job,
            JobEvent::ResolveAfterCleanup(ConflictResolution::Restart),
            2,
        )
        .unwrap();

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(result, RunnerResult::Completed(_)));
        let truncate = operations
            .iter()
            .position(|entry| entry == "truncate_partial")
            .unwrap();
        let open = operations
            .iter()
            .position(|entry| entry == "open_partial")
            .unwrap();
        assert!(truncate < open);
    }

    #[tokio::test]
    async fn ask_preserves_an_existing_final_and_reports_resume_only_for_durable_partial() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: false,
                backup_exists: false,
            },
            None,
        );

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::DestinationConflict {
                resume_available: false
            })
        ));
        assert_eq!(
            operations,
            vec![
                "resolve",
                "checking",
                "open_source",
                "fingerprinted",
                "inventory",
                "finish_source"
            ]
        );
    }

    #[tokio::test]
    async fn ask_rechecks_and_preserves_a_final_that_appears_during_copy() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_final_appearing_after_copy();

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::DestinationConflict {
                resume_available: true
            })
        ));
        assert_eq!(
            operations
                .iter()
                .filter(|entry| *entry == "inventory")
                .count(),
            2
        );
        assert!(!operations.iter().any(|entry| entry == "phase:Prepared"));
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "move_final_to_backup")
        );
        assert!(!operations.iter().any(|entry| entry == "promote_partial"));
    }

    #[tokio::test]
    async fn overwrite_rechecks_and_backs_up_a_final_that_appears_during_copy() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_final_appearing_after_copy();
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(result, RunnerResult::Completed(_)));
        assert_eq!(
            operations
                .iter()
                .filter(|entry| *entry == "inventory")
                .count(),
            2
        );
        let moved = operations
            .iter()
            .position(|entry| entry == "move_final_to_backup")
            .unwrap();
        let promoted = operations
            .iter()
            .position(|entry| entry == "promote_partial")
            .unwrap();
        assert!(moved < promoted);
    }

    #[tokio::test]
    async fn ask_no_replace_promotion_preserves_a_final_that_appears_after_prepared() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io =
            FakeIo::new(log, fingerprint(4, Some("v1"))).with_final_appearing_before_promotion();
        let inspected_io = io.clone();

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::DestinationConflict {
                resume_available: true
            })
        ));
        assert_eq!(
            inspected_io.artifact_identities(),
            (Some("late-final"), Some("copied-source"), None)
        );
        assert!(operations.iter().any(|entry| entry == "phase:Prepared"));
        assert!(operations.iter().any(|entry| entry == "phase:BackupMoved"));
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "phase:PartialPromoted")
        );
    }

    #[tokio::test]
    async fn overwrite_no_replace_promotion_preserves_a_final_that_appears_after_backup_move() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1")))
            .with_inventory(
                ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: false,
                },
                None,
            )
            .with_final_appearing_before_promotion();
        let inspected_io = io.clone();
        let mut job = test_job();
        job.conflict_policy = ConflictPolicy::Overwrite;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        let RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { message }) = result
        else {
            panic!("late overwrite final must require conservative commit recovery")
        };
        assert!(
            message.contains("final=true, partial=true, backup=true"),
            "unexpected recovery inventory: {message}"
        );
        assert_eq!(
            inspected_io.artifact_identities(),
            (
                Some("late-final"),
                Some("copied-source"),
                Some("existing-final")
            )
        );
        assert!(operations.iter().any(|entry| entry == "phase:Prepared"));
        assert!(operations.iter().any(|entry| entry == "phase:BackupMoved"));
        assert!(!operations.iter().any(|entry| entry == "restore_backup"));
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "phase:PartialPromoted")
        );
    }

    #[tokio::test]
    async fn ambiguous_local_promotion_preserves_both_links_and_requires_inventory_attention() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_ambiguous_promotion();
        let inspected_io = io.clone();

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Run).await;

        let RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { message }) = result
        else {
            panic!("ambiguous promotion must require exact-inventory recovery")
        };
        assert!(
            message.contains("final=true, partial=true, backup=false"),
            "unexpected recovery inventory: {message}"
        );
        assert_eq!(
            inspected_io.artifact_identities(),
            (Some("copied-source"), Some("copied-source"), None)
        );
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "phase:PartialPromoted")
        );
        assert!(!operations.iter().any(|entry| entry == "delete_backup"));
    }

    #[tokio::test]
    async fn overwrite_resolution_redispatches_through_the_runner_without_conflict_looping() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(4, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: false,
                backup_exists: false,
            },
            None,
        );
        let mut job = test_job();
        job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: false,
            },
        };
        let job = reduce_job(&job, JobEvent::Resolve(ConflictResolution::Overwrite), 2).unwrap();

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Completed(super::CompletionResult::Transferred)
        ));
        assert!(
            operations
                .iter()
                .any(|entry| entry == "move_final_to_backup")
        );
    }

    #[tokio::test]
    async fn resume_resolution_redispatches_owned_checkpoint_without_conflict_looping() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: true,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        let mut job = test_job();
        job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::DestinationConflict {
                resume_available: true,
            },
        };
        job.source_fingerprint = Some(fingerprint(6, Some("v1")));
        job.durable_checkpoint = 2;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        let job = reduce_job(&job, JobEvent::Resolve(ConflictResolution::Resume), 2).unwrap();

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Completed(super::CompletionResult::Transferred)
        ));
        assert!(operations.iter().any(|entry| entry == "partial_size"));
        assert!(
            operations
                .iter()
                .any(|entry| entry == "move_final_to_backup")
        );
    }

    #[tokio::test]
    async fn longer_partial_is_truncated_to_durable_checkpoint_before_resume() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(5),
        );
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(6, Some("v1")));
        job.durable_checkpoint = 3;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(result, RunnerResult::Completed(_)));
        let truncate = operations
            .iter()
            .position(|entry| entry == "truncate_partial")
            .unwrap();
        let open = operations
            .iter()
            .position(|entry| entry == "open_partial")
            .unwrap();
        assert!(truncate < open);
        assert!(!operations.iter().any(|entry| entry == "checkpoint:5"));
    }

    #[tokio::test]
    async fn shorter_partial_lowers_the_durable_checkpoint_before_copying() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1"))).with_inventory(
            ArtifactInventory {
                final_exists: false,
                partial_exists: true,
                backup_exists: false,
            },
            Some(2),
        );
        let mut job = test_job();
        job.source_fingerprint = Some(fingerprint(6, Some("v1")));
        job.durable_checkpoint = 4;
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Run).await;

        assert!(matches!(result, RunnerResult::Completed(_)));
        let lowered = operations
            .iter()
            .position(|entry| entry == "checkpoint:2")
            .unwrap();
        let open = operations
            .iter()
            .position(|entry| entry == "open_partial")
            .unwrap();
        assert!(lowered < open);
    }

    #[tokio::test]
    async fn pause_flushes_and_checkpoints_but_preserves_the_partial() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1")));

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Pause).await;

        assert!(matches!(
            result,
            RunnerResult::Paused {
                durable_checkpoint: 2
            }
        ));
        assert!(operations.iter().any(|entry| entry == "checkpoint:2"));
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "cleanup_owned_artifacts")
        );
        assert!(!operations.iter().any(|entry| entry == "phase:Prepared"));
    }

    #[tokio::test]
    async fn inactive_cancel_cleans_owned_artifacts_without_reopening_the_transfer() {
        let states = [TransferJobState::Queued, TransferJobState::Paused];
        for state in states {
            let log = Arc::new(StdMutex::new(Vec::new()));
            let io = FakeIo::new(log, fingerprint(6, Some("v1")));
            let mut job = test_job();
            job.state = state;
            job.durable_checkpoint = 2;
            job.artifacts =
                Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());

            let (result, operations) =
                run_fake(true, io, job, super::RunnerControlState::Cancel).await;

            assert!(matches!(
                result,
                RunnerResult::Cancelled {
                    cleanup_error: None
                }
            ));
            assert!(
                operations
                    .iter()
                    .any(|entry| entry == "cleanup_owned_artifacts")
            );
            assert!(!operations.iter().any(|entry| entry == "open_source"));
            assert!(!operations.iter().any(|entry| entry == "open_partial"));
        }
    }

    #[tokio::test]
    async fn inactive_cancel_preserves_a_destination_for_each_recoverable_commit_layout() {
        struct Case {
            phase: CommitPhase,
            backup_expected: Option<bool>,
            inventory: ArtifactInventory,
            expected_identities: (
                Option<&'static str>,
                Option<&'static str>,
                Option<&'static str>,
            ),
            required_operation: Option<&'static str>,
        }

        let cases = [
            Case {
                phase: CommitPhase::None,
                backup_expected: None,
                inventory: ArtifactInventory {
                    final_exists: true,
                    partial_exists: true,
                    backup_exists: false,
                },
                expected_identities: (Some("existing-final"), None, None),
                required_operation: Some("cleanup_owned_artifacts"),
            },
            Case {
                phase: CommitPhase::Prepared,
                backup_expected: Some(true),
                inventory: ArtifactInventory {
                    final_exists: true,
                    partial_exists: true,
                    backup_exists: false,
                },
                expected_identities: (Some("existing-final"), None, None),
                required_operation: Some("cleanup_owned_artifacts"),
            },
            Case {
                phase: CommitPhase::Prepared,
                backup_expected: Some(false),
                inventory: ArtifactInventory {
                    final_exists: false,
                    partial_exists: true,
                    backup_exists: false,
                },
                expected_identities: (Some("copied-source"), None, None),
                required_operation: Some("promote_partial"),
            },
            Case {
                phase: CommitPhase::BackupMoved,
                backup_expected: Some(true),
                inventory: ArtifactInventory {
                    final_exists: false,
                    partial_exists: false,
                    backup_exists: true,
                },
                expected_identities: (Some("owned-backup"), None, None),
                required_operation: Some("restore_backup"),
            },
            Case {
                phase: CommitPhase::PartialPromoted,
                backup_expected: Some(true),
                inventory: ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: true,
                },
                expected_identities: (Some("existing-final"), None, None),
                required_operation: Some("delete_backup"),
            },
            Case {
                phase: CommitPhase::PartialPromoted,
                backup_expected: Some(false),
                inventory: ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: false,
                },
                expected_identities: (Some("existing-final"), None, None),
                required_operation: None,
            },
        ];

        for case in cases {
            let log = Arc::new(StdMutex::new(Vec::new()));
            let io = FakeIo::new(log, fingerprint(6, Some("v1")))
                .with_inventory(case.inventory, case.inventory.partial_exists.then_some(6));
            let inspected_io = io.clone();
            let mut job = test_job();
            job.state = TransferJobState::NeedsAttention {
                reason: AttentionReason::CommitRecovery {
                    message: "recover before cancellation".into(),
                },
            };
            job.durable_checkpoint = 6;
            job.artifacts =
                Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
            job.commit_phase = case.phase;
            job.commit_backup_expected = case.backup_expected;

            let (result, operations) =
                run_fake(true, io, job, super::RunnerControlState::Cancel).await;

            assert!(matches!(
                result,
                RunnerResult::Cancelled {
                    cleanup_error: None
                }
            ));
            assert_eq!(
                inspected_io.artifact_identities(),
                case.expected_identities,
                "phase {:?}, inventory {:?}, operations {operations:?}",
                case.phase,
                case.inventory
            );
            assert!(operations.iter().any(|entry| entry == "inventory"));
            if let Some(required) = case.required_operation {
                assert!(
                    operations.iter().any(|entry| entry == required),
                    "phase {:?}, inventory {:?}, operations {operations:?}",
                    case.phase,
                    case.inventory
                );
            }
        }
    }

    #[tokio::test]
    async fn inactive_cancel_quarantines_unproven_prepared_final_and_backup_for_all_provenance() {
        for backup_expected in [None, Some(false), Some(true)] {
            let log = Arc::new(StdMutex::new(Vec::new()));
            let io = FakeIo::new(log, fingerprint(6, Some("v1"))).with_inventory(
                ArtifactInventory {
                    final_exists: true,
                    partial_exists: false,
                    backup_exists: true,
                },
                None,
            );
            let inspected_io = io.clone();
            let mut job = test_job();
            job.state = TransferJobState::NeedsAttention {
                reason: AttentionReason::CommitRecovery {
                    message: "unproven prepared layout".into(),
                },
            };
            job.artifacts =
                Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
            job.commit_phase = CommitPhase::Prepared;
            job.commit_backup_expected = backup_expected;

            let (result, operations) = run_fake(true, io, job, RunnerControlState::Cancel).await;

            assert!(matches!(
                result,
                RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { .. })
            ));
            assert_eq!(
                inspected_io.artifact_identities(),
                (Some("existing-final"), None, Some("owned-backup")),
                "provenance {backup_expected:?}, operations {operations:?}"
            );
            assert_eq!(operations, vec!["resolve", "inventory"]);
        }
    }

    #[tokio::test]
    async fn inactive_cancel_never_touches_mismatched_persisted_artifact_paths() {
        for persisted in [
            None,
            Some(
                ManagedArtifacts::for_destination(Uuid::from_u128(0xbbbb), "/srv/final.bin")
                    .unwrap(),
            ),
        ] {
            let log = Arc::new(StdMutex::new(Vec::new()));
            let io = FakeIo::new(log, fingerprint(6, Some("v1"))).with_inventory(
                ArtifactInventory {
                    final_exists: false,
                    partial_exists: false,
                    backup_exists: true,
                },
                None,
            );
            let inspected_io = io.clone();
            let mut job = test_job();
            job.state = TransferJobState::Paused;
            job.durable_checkpoint = 6;
            job.artifacts = persisted;
            job.commit_phase = CommitPhase::BackupMoved;

            let (result, operations) =
                run_fake(true, io, job, super::RunnerControlState::Cancel).await;

            assert!(matches!(
                result,
                RunnerResult::NeedsAttention(AttentionReason::CommitRecovery { .. })
            ));
            assert!(
                operations.is_empty(),
                "mismatch performed I/O: {operations:?}"
            );
            assert_eq!(
                inspected_io.artifact_identities(),
                (None, None, Some("owned-backup"))
            );
        }
    }

    #[tokio::test]
    async fn inactive_cancel_quarantines_a_backup_when_restore_fails() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1")))
            .with_inventory(
                ArtifactInventory {
                    final_exists: false,
                    partial_exists: false,
                    backup_exists: true,
                },
                None,
            )
            .failing_at("restore_backup");
        let inspected_io = io.clone();
        let mut job = test_job();
        job.state = TransferJobState::NeedsAttention {
            reason: AttentionReason::CommitRecovery {
                message: "restore required".into(),
            },
        };
        job.artifacts = Some(ManagedArtifacts::for_destination(job.id, &job.remote_path).unwrap());
        job.commit_phase = CommitPhase::BackupMoved;

        let (result, operations) = run_fake(true, io, job, super::RunnerControlState::Cancel).await;

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::Cleanup { .. })
        ));
        assert_eq!(
            inspected_io.artifact_identities(),
            (None, None, Some("owned-backup"))
        );
        assert!(operations.iter().any(|entry| entry == "restore_backup"));
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "cleanup_owned_artifacts")
        );
    }

    #[tokio::test]
    async fn cancel_removes_only_uuid_owned_artifacts_after_close_and_checkpoint() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1")));

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Cancel).await;

        assert!(matches!(
            result,
            RunnerResult::Cancelled {
                cleanup_error: None
            }
        ));
        let finish = operations
            .iter()
            .position(|entry| entry == "finish_partial")
            .unwrap();
        let checkpoint = operations
            .iter()
            .position(|entry| entry == "checkpoint:2")
            .unwrap();
        let cleanup = operations
            .iter()
            .position(|entry| entry == "cleanup_owned_artifacts")
            .unwrap();
        assert!(finish < checkpoint && checkpoint < cleanup);
    }

    #[tokio::test]
    async fn disk_failure_is_permanent_and_preserves_the_last_durable_artifacts() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1"))).failing_at("finish_partial");

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Failed {
                class: FailureClass::Permanent,
                ..
            }
        ));
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "cleanup_owned_artifacts")
        );
        assert!(!operations.iter().any(|entry| entry.starts_with("phase:")));
    }

    #[tokio::test]
    async fn permission_failure_is_permanent_and_preserves_owned_artifacts() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1"))).failing_at_with(
            "finish_partial",
            TransferIoError::permanent("permission denied while syncing the owned partial"),
        );
        let inspected_io = io.clone();

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Failed {
                class: FailureClass::Permanent,
                ref message,
            } if message.contains("permission denied")
        ));
        assert_eq!(
            inspected_io.artifact_identities(),
            (None, Some("copied-source"), None)
        );
        assert!(
            !operations
                .iter()
                .any(|entry| entry == "cleanup_owned_artifacts")
        );
        assert!(!operations.iter().any(|entry| entry.starts_with("phase:")));
    }

    #[tokio::test]
    async fn disconnect_is_classified_transient_without_touching_artifacts() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let io = FakeIo::new(log, fingerprint(6, Some("v1"))).failing_at("open_source");

        let (result, operations) =
            run_fake(true, io, test_job(), super::RunnerControlState::Run).await;

        assert!(matches!(
            result,
            RunnerResult::Failed {
                class: FailureClass::Transient,
                ..
            }
        ));
        assert_eq!(operations, vec!["resolve", "checking", "open_source"]);
    }

    #[tokio::test]
    async fn missing_source_needs_attention_and_restart_succeeds_after_restoration() {
        for direction in [TransferDirection::Upload, TransferDirection::Download] {
            let log = Arc::new(StdMutex::new(Vec::new()));
            let io = FakeIo::new(log, fingerprint(6, Some("v1"))).failing_at_with(
                "open_source",
                TransferIoError::source_missing("source is no longer present"),
            );
            let mut job = test_job();
            job.direction = direction;

            let (result, operations) = run_fake(
                true,
                io.clone(),
                job.clone(),
                super::RunnerControlState::Run,
            )
            .await;

            assert!(matches!(
                result,
                RunnerResult::NeedsAttention(AttentionReason::SourceMissing)
            ));
            assert_eq!(operations, vec!["resolve", "checking", "open_source"]);

            job.state = TransferJobState::NeedsAttention {
                reason: AttentionReason::SourceMissing,
            };
            let restarted =
                reduce_job(&job, JobEvent::Resolve(ConflictResolution::Restart), 2).unwrap();
            let (restored_result, _) =
                run_fake(true, io, restarted, super::RunnerControlState::Run).await;
            assert!(matches!(restored_result, RunnerResult::Completed(_)));
        }
    }

    #[tokio::test]
    async fn real_local_upload_source_missing_maps_to_source_missing_attention() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-upload-source.bin");

        let error = fingerprint_open_local(&missing).await.unwrap_err();
        let result = failed(TransferIoError::from_local_source(error));

        assert!(matches!(
            result,
            RunnerResult::NeedsAttention(AttentionReason::SourceMissing)
        ));
    }

    #[test]
    fn closed_sftp_channel_is_a_transient_disconnect() {
        let error = TransferIoError::from_remote(RemoteError::Sftp(
            "the SFTP channel closed while reading".into(),
        ));

        assert_eq!(error.class, FailureClass::Transient);
    }

    #[test]
    fn contextual_sftp_operation_preserves_disconnect_classification() {
        let error = TransferIoError::from_remote_operation(
            "inspect remote artifact",
            "channel closed by server",
        );

        assert_eq!(error.class, FailureClass::Transient);
        assert_eq!(
            error.message,
            "inspect remote artifact failed: channel closed by server"
        );
    }

    #[test]
    fn local_permission_failure_stays_permanent_when_its_path_says_timeout_and_disconnect() {
        let error = TransferIoError::from_operation(
            FailureClass::Permanent,
            "open local source /tmp/timeout/disconnect.bin",
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
        );

        assert_eq!(error.class, FailureClass::Permanent);
        assert!(error.message.contains("timeout/disconnect.bin"));

        let already_contextualized = TransferIoError::from_remote(RemoteError::Transfer(
            "open local source /tmp/timeout/disconnect.bin failed: permission denied".into(),
        ));
        assert_eq!(already_contextualized.class, FailureClass::Permanent);
    }

    #[tokio::test]
    async fn upload_local_read_failure_cannot_be_made_transient_by_path_text() {
        let error = classify_copy_read_failure(
            TransferDirection::Upload,
            ErrorKind::Other,
            "read /tmp/timeout/disconnect.bin failed",
        )
        .await;

        assert_eq!(error.class, FailureClass::Permanent);
    }

    #[tokio::test]
    async fn download_local_write_failure_cannot_be_made_transient_by_path_text() {
        let error = classify_copy_write_failure(
            TransferDirection::Download,
            ErrorKind::PermissionDenied,
            "write /tmp/timeout/disconnect.bin failed",
        )
        .await;

        assert_eq!(error.class, FailureClass::Permanent);
    }

    #[tokio::test]
    async fn structured_remote_read_and_write_disconnects_are_transient() {
        let read = classify_copy_read_failure(
            TransferDirection::Download,
            ErrorKind::ConnectionReset,
            "peer reset the channel",
        )
        .await;
        let write = classify_copy_write_failure(
            TransferDirection::Upload,
            ErrorKind::BrokenPipe,
            "remote pipe closed",
        )
        .await;

        assert_eq!(read.class, FailureClass::Transient);
        assert_eq!(write.class, FailureClass::Transient);
    }

    #[tokio::test]
    async fn whole_raw_channel_closed_cause_remains_transient() {
        let error = classify_copy_read_failure(
            TransferDirection::Download,
            ErrorKind::Other,
            "channel closed: server reason",
        )
        .await;

        assert_eq!(error.class, FailureClass::Transient);
    }

    #[tokio::test]
    async fn structured_remote_permission_failure_is_permanent() {
        let error = classify_copy_write_failure(
            TransferDirection::Upload,
            ErrorKind::PermissionDenied,
            "permission denied",
        )
        .await;

        assert_eq!(error.class, FailureClass::Permanent);
    }

    #[test]
    fn untrusted_transfer_text_defaults_to_permanent_even_when_it_says_channel_closed() {
        let error = TransferIoError::from_remote(RemoteError::Transfer(
            "copy /tmp/channel closed failed".into(),
        ));

        assert_eq!(error.class, FailureClass::Permanent);
    }

    #[test]
    fn structured_remote_disconnect_is_transient() {
        let error = TransferIoError::from_remote(RemoteError::Connection(
            "remote peer disconnected".into(),
        ));

        assert_eq!(error.class, FailureClass::Transient);
    }

    #[test]
    fn configured_connection_lookup_uses_only_the_exact_server_entry_id() {
        let connections = [
            LiveConnectionIdentity {
                key: "alice",
                server_entry_id: Some("server-a"),
                host: "shared.example.com",
                port: 22,
                user: "alice",
            },
            LiveConnectionIdentity {
                key: "bob",
                server_entry_id: Some("server-b"),
                host: "shared.example.com",
                port: 22,
                user: "bob",
            },
        ];

        let selected = select_live_connection_key(
            connections,
            &TransferEndpoint::Configured {
                server_entry_id: "server-b".into(),
                label: "Production".into(),
            },
        );

        assert_eq!(selected, Some("bob"));
    }

    #[test]
    fn ad_hoc_connection_lookup_requires_exact_host_port_and_user() {
        let connections = [
            LiveConnectionIdentity {
                key: "right",
                server_entry_id: None,
                host: "files.example.com",
                port: 2222,
                user: "deploy",
            },
            LiveConnectionIdentity {
                key: "wrong-user",
                server_entry_id: None,
                host: "files.example.com",
                port: 2222,
                user: "root",
            },
            LiveConnectionIdentity {
                key: "wrong-port",
                server_entry_id: None,
                host: "files.example.com",
                port: 22,
                user: "deploy",
            },
        ];

        let endpoint = |port, user: &str| TransferEndpoint::AdHoc {
            host: "files.example.com".into(),
            port,
            user: user.into(),
            proxy_command: None,
            proxy_jump: None,
        };

        assert_eq!(
            select_live_connection_key(connections, &endpoint(2222, "deploy")),
            Some("right")
        );
        assert_eq!(
            select_live_connection_key(connections, &endpoint(2222, "missing")),
            None
        );
        assert_eq!(
            select_live_connection_key(connections, &endpoint(2200, "deploy")),
            None
        );
    }

    #[test]
    fn connection_lookup_returns_none_when_no_live_identity_matches() {
        let connections = [LiveConnectionIdentity {
            key: "unrelated",
            server_entry_id: Some("another-server"),
            host: "elsewhere.example.com",
            port: 22,
            user: "root",
        }];

        assert_eq!(
            select_live_connection_key(
                connections,
                &TransferEndpoint::Configured {
                    server_entry_id: "missing".into(),
                    label: "Missing".into(),
                }
            ),
            None
        );
    }

    #[tokio::test]
    async fn progress_burst_keeps_one_wake_and_converges_to_the_latest_value() {
        let job_id = Uuid::from_u128(91);
        let lease_id = Uuid::from_u128(92);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let reporter = RunnerReporter::new(job_id, lease_id, event_tx);

        for bytes in 1..=10_000 {
            reporter.progress(bytes, Some(bytes * 2), Some(10_001 - bytes));
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
        assert_eq!(latest.speed_bytes_per_second, Some(20_000));
        assert_eq!(latest.eta_seconds, Some(1));
        assert!(event_rx.is_empty());

        reporter.progress(10_001, Some(30_000), None);
        assert_eq!(event_rx.len(), 1, "draining re-arms exactly one wake");
        let RunnerEvent::ProgressReady { slot, .. } = event_rx.recv().await.unwrap() else {
            panic!("progress publishes a progress-ready wake")
        };
        assert_eq!(slot.take_latest_and_release_wake().unwrap().bytes, 10_001);
    }
}
