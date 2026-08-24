use std::{
    fmt,
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
        copy_with_checkpoint, fingerprint_open_local, fingerprint_open_remote, open_local_partial,
        open_remote_partial, open_sftp_session, truncate_local_partial, truncate_remote_partial,
    },
};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use super::{
    artifacts::ArtifactInventory,
    events::RunnerEvent,
    model::{
        AttentionReason, CommitPhase, CompletionResult, ConflictPolicy, ManagedArtifacts,
        TransferDirection, TransferEndpoint, TransferJob,
    },
    scheduler::FailureClass,
};
use crate::remote::RemoteState;

const DEFAULT_COPY_CHUNK_SIZE: usize = 256 * 1024;

#[derive(Clone)]
pub(crate) struct ResolvedSftpConnection {
    pub(crate) ssh_handle:
        Arc<termlab_remote::russh::client::Handle<termlab_remote::handler::TermLabSshHandler>>,
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
}

impl TransferIoError {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Transient,
            message: message.into(),
        }
    }

    fn permanent(message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Permanent,
            message: message.into(),
        }
    }

    fn from_remote(error: RemoteError) -> Self {
        let class = match &error {
            RemoteError::Connection(_) => FailureClass::Transient,
            RemoteError::Io(error) => classify_io_failure(error.kind()),
            RemoteError::Sftp(message) | RemoteError::Transfer(message) => {
                classify_transport_failure(message)
            }
            RemoteError::Auth(_)
            | RemoteError::Tunnel(_)
            | RemoteError::KnownHosts(_)
            | RemoteError::Other(_) => FailureClass::Permanent,
        };
        let message = error.to_string();
        Self { class, message }
    }

    fn from_operation(class: FailureClass, operation: &str, error: impl fmt::Display) -> Self {
        Self {
            class,
            message: format!("{operation} failed: {error}"),
        }
    }

    fn from_local(error: impl fmt::Display) -> Self {
        Self::permanent(error.to_string())
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
}

fn classify_io_failure(kind: std::io::ErrorKind) -> FailureClass {
    match kind {
        std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::TimedOut
        | std::io::ErrorKind::UnexpectedEof => FailureClass::Transient,
        _ => FailureClass::Permanent,
    }
}

/// Classify only the raw error supplied by remote transport infrastructure.
/// Operation context and user-controlled paths must be appended after this.
fn classify_transport_failure(error: &str) -> FailureClass {
    let error = error.to_ascii_lowercase();
    let cause = error
        .rsplit_once(": ")
        .map_or(error.as_str(), |(_, cause)| cause);
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

#[async_trait]
trait TransferIo<C>: Send + Sync {
    type Source: AsyncRead + AsyncSeek + Unpin + Send;
    type Partial: AsyncWrite + AsyncSeek + Unpin + Send;

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
    async fn promote_partial(
        &self,
        connection: &C,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError>;
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
    ) -> RunnerResult;
}

struct RunnerServices<R, I> {
    resolver: R,
    io: I,
    chunk_size: usize,
}

pub(crate) struct SftpTransferJobRunner {
    attempt: Arc<dyn SftpAttempt>,
}

impl SftpTransferJobRunner {
    pub(crate) fn new(remote: Arc<ParkingMutex<RemoteState>>) -> Self {
        Self::with_services(
            LiveConnectionResolver { remote },
            RealTransferIo,
            DEFAULT_COPY_CHUNK_SIZE,
        )
    }

    fn with_services<R, I>(resolver: R, io: I, chunk_size: usize) -> Self
    where
        R: ConnectionResolver + 'static,
        I: TransferIo<R::Connection> + 'static,
    {
        Self {
            attempt: Arc::new(RunnerServices {
                resolver,
                io,
                chunk_size,
            }),
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
    ) -> RunnerResult {
        self.attempt.run(job, control, reporter).await
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
    ) -> RunnerResult {
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
        let copy_result = copy_with_checkpoint(
            &mut source,
            &mut partial,
            offset,
            actual.size,
            self.chunk_size,
            || match control.state() {
                RunnerControlState::Run => ControlDecision::Continue,
                RunnerControlState::Pause => ControlDecision::Pause,
                RunnerControlState::Cancel => ControlDecision::Cancel,
            },
            |bytes, total| {
                let remaining = total.saturating_sub(bytes);
                reporter.progress(bytes, 0, (remaining > 0).then_some(0));
            },
        )
        .await
        .map_err(TransferIoError::from_remote);

        let source_finish = self.io.finish_source(&mut source).await;
        let partial_finish = self.io.finish_partial(&mut partial).await;
        let outcome = match copy_result {
            Ok(outcome) => outcome,
            Err(error) => return failed(error),
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
    RunnerResult::Failed {
        class: error.class,
        message: error.message,
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
    if let Err(message) = reporter.commit_phase(CommitPhase::Prepared).await {
        return permanent_failure(message);
    }
    if final_exists && let Err(error) = io.move_final_to_backup(connection, job, artifacts).await {
        return commit_attention(io, connection, job, artifacts, error.message).await;
    }
    if let Err(message) = reporter.commit_phase(CommitPhase::BackupMoved).await {
        return permanent_failure(message);
    }
    if let Err(error) = io.promote_partial(connection, job, artifacts).await {
        if final_exists {
            let _ = io.restore_backup(connection, job, artifacts).await;
        }
        return commit_attention(io, connection, job, artifacts, error.message).await;
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
    let mut inventory = match io.inventory(connection, job, artifacts).await {
        Ok(inventory) => inventory,
        Err(error) => return failed(error),
    };
    let mut phase = job.commit_phase;

    if phase == CommitPhase::Prepared {
        if !inventory.partial_exists {
            if !inventory.final_exists && inventory.backup_exists {
                let _ = io.restore_backup(connection, job, artifacts).await;
            }
            return commit_attention(
                io,
                connection,
                job,
                artifacts,
                "Prepared commit has no promotable partial",
            )
            .await;
        }
        if inventory.final_exists && !inventory.backup_exists {
            if let Err(error) = io.move_final_to_backup(connection, job, artifacts).await {
                return commit_attention(io, connection, job, artifacts, error.message).await;
            }
        } else if inventory.final_exists && inventory.backup_exists {
            return commit_attention(
                io,
                connection,
                job,
                artifacts,
                "Prepared commit has both final and backup",
            )
            .await;
        }
        if let Err(message) = reporter.commit_phase(CommitPhase::BackupMoved).await {
            return permanent_failure(message);
        }
        phase = CommitPhase::BackupMoved;
        inventory = match io.inventory(connection, job, artifacts).await {
            Ok(inventory) => inventory,
            Err(error) => return failed(error),
        };
    }

    if phase == CommitPhase::BackupMoved {
        if inventory.partial_exists && !inventory.final_exists {
            if let Err(error) = io.promote_partial(connection, job, artifacts).await {
                if inventory.backup_exists {
                    let _ = io.restore_backup(connection, job, artifacts).await;
                }
                return commit_attention(io, connection, job, artifacts, error.message).await;
            }
        } else if !inventory.partial_exists && inventory.final_exists {
            // Promotion completed before its durable phase acknowledgement.
        } else {
            if !inventory.final_exists && inventory.backup_exists {
                let _ = io.restore_backup(connection, job, artifacts).await;
            }
            return commit_attention(
                io,
                connection,
                job,
                artifacts,
                "BackupMoved inventory cannot prove a completed promotion",
            )
            .await;
        }
        if let Err(message) = reporter.commit_phase(CommitPhase::PartialPromoted).await {
            return permanent_failure(message);
        }
        phase = CommitPhase::PartialPromoted;
        inventory = match io.inventory(connection, job, artifacts).await {
            Ok(inventory) => inventory,
            Err(error) => return failed(error),
        };
    }

    if matches!(
        phase,
        CommitPhase::PartialPromoted | CommitPhase::CleanupPending
    ) {
        if !inventory.final_exists || inventory.partial_exists {
            if !inventory.final_exists && inventory.backup_exists {
                let _ = io.restore_backup(connection, job, artifacts).await;
            }
            return commit_attention(
                io,
                connection,
                job,
                artifacts,
                "PartialPromoted inventory does not contain exactly one promoted final",
            )
            .await;
        }
        if inventory.backup_exists
            && let Err(error) = io.delete_backup(connection, job, artifacts).await
        {
            return commit_attention(io, connection, job, artifacts, error.message).await;
        }
        if let Err(message) = reporter.commit_phase(CommitPhase::Complete).await {
            return permanent_failure(message);
        }
        return RunnerResult::Completed(CompletionResult::Transferred);
    }

    if phase == CommitPhase::Complete
        && inventory.final_exists
        && !inventory.partial_exists
        && !inventory.backup_exists
    {
        return RunnerResult::Completed(CompletionResult::Transferred);
    }

    commit_attention(
        io,
        connection,
        job,
        artifacts,
        format!("unexpected persisted commit phase {phase:?}"),
    )
    .await
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

struct RealTransferIo;

enum RealSource {
    Local(tokio::fs::File),
    Remote(SftpFileHandle),
}

enum RealPartial {
    Local(tokio::fs::File),
    Remote(SftpFileHandle),
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
        match self.get_mut() {
            Self::Local(file) => Pin::new(file).poll_write(context, buffer),
            Self::Remote(file) => Pin::new(file).poll_write(context, buffer),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Local(file) => Pin::new(file).poll_flush(context),
            Self::Remote(file) => Pin::new(file).poll_flush(context),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Local(file) => Pin::new(file).poll_shutdown(context),
            Self::Remote(file) => Pin::new(file).poll_shutdown(context),
        }
    }
}

impl AsyncSeek for RealPartial {
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
                .map_err(TransferIoError::from_local),
            TransferDirection::Download => {
                let session = open_sftp_session(&connection.ssh_handle)
                    .await
                    .map_err(TransferIoError::from_remote)?;
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
                let session = open_sftp_session(&connection.ssh_handle)
                    .await
                    .map_err(TransferIoError::from_remote)?;
                Ok(ArtifactInventory {
                    final_exists: remote_exists(&session, &job.remote_path).await?,
                    partial_exists: remote_exists(&session, &artifacts.partial_path).await?,
                    backup_exists: remote_exists(&session, &artifacts.backup_path).await?,
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
                let session = open_sftp_session(&connection.ssh_handle)
                    .await
                    .map_err(TransferIoError::from_remote)?;
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
                let session = open_sftp_session(&connection.ssh_handle)
                    .await
                    .map_err(TransferIoError::from_remote)?;
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
                let session = open_sftp_session(&connection.ssh_handle)
                    .await
                    .map_err(TransferIoError::from_remote)?;
                open_remote_partial(&session, &artifacts.partial_path, resume)
                    .await
                    .map(RealPartial::Remote)
                    .map_err(TransferIoError::from_remote)
            }
            TransferDirection::Download => open_local_partial(&artifacts.partial_path, resume)
                .await
                .map(RealPartial::Local)
                .map_err(TransferIoError::from_local),
        }
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
        match partial {
            RealPartial::Local(file) => {
                file.flush().await.map_err(|error| {
                    TransferIoError::permanent(format!("flush local partial failed: {error}"))
                })?;
                file.sync_all().await.map_err(|error| {
                    TransferIoError::permanent(format!("sync local partial failed: {error}"))
                })
            }
            RealPartial::Remote(file) => {
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

    async fn promote_partial(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError> {
        rename_artifact(connection, job, &artifacts.partial_path, final_path(job)).await
    }

    async fn restore_backup(
        &self,
        connection: &ResolvedSftpConnection,
        job: &TransferJob,
        artifacts: &ManagedArtifacts,
    ) -> Result<(), TransferIoError> {
        rename_artifact(connection, job, &artifacts.backup_path, final_path(job)).await
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
            let session = open_sftp_session(&connection.ssh_handle)
                .await
                .map_err(TransferIoError::from_remote)?;
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

async fn remove_artifact(
    connection: &ResolvedSftpConnection,
    job: &TransferJob,
    path: &str,
    missing_ok: bool,
) -> Result<(), TransferIoError> {
    match job.direction {
        TransferDirection::Upload => {
            let session = open_sftp_session(&connection.ssh_handle)
                .await
                .map_err(TransferIoError::from_remote)?;
            if missing_ok && !remote_exists(&session, path).await? {
                return Ok(());
            }
            session.remove_file(path.to_owned()).await.map_err(|error| {
                TransferIoError::from_remote_operation("delete remote artifact", error)
            })
        }
        TransferDirection::Download => {
            if missing_ok && !local_exists(path).await? {
                return Ok(());
            }
            tokio::fs::remove_file(path).await.map_err(|error| {
                TransferIoError::permanent(format!("delete local artifact failed: {error}"))
            })
        }
    }
}

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
    use std::{
        io::Cursor,
        sync::{Arc, Mutex as StdMutex},
    };

    use async_trait::async_trait;
    use termlab_remote::{RemoteError, transfer::SourceFingerprint};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::{
        ConnectionResolver, LiveConnectionIdentity, RunnerControl, RunnerReporter, RunnerResult,
        SftpTransferJobRunner, TransferIo, TransferIoError, TransferJobRunner,
        select_live_connection_key,
    };
    use crate::remote::transfer_queue::{
        artifacts::ArtifactInventory,
        events::RunnerEvent,
        model::{
            AttentionReason, CommitPhase, ConflictPolicy, ConflictResolution, ManagedArtifacts,
            TransferDirection, TransferEndpoint, TransferJob, TransferJobState, TransferOrigin,
            TransferPriority, TransferProtocol,
        },
        reducer::{JobEvent, reduce_job},
        scheduler::FailureClass,
    };

    #[derive(Clone, Copy)]
    struct TestConnection;

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
    }

    struct FakeIoState {
        log: Arc<StdMutex<Vec<String>>>,
        fingerprint: SourceFingerprint,
        inventory: ArtifactInventory,
        partial_len: Option<u64>,
        final_after_finish_partial: bool,
        fail_at: Option<&'static str>,
    }

    impl FakeIo {
        fn new(log: Arc<StdMutex<Vec<String>>>, fingerprint: SourceFingerprint) -> Self {
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
                    fail_at: None,
                })),
            }
        }

        fn with_inventory(self, inventory: ArtifactInventory, partial_len: Option<u64>) -> Self {
            {
                let mut state = self.state.lock().unwrap();
                state.inventory = inventory;
                state.partial_len = partial_len;
            }
            self
        }

        fn failing_at(self, operation: &'static str) -> Self {
            self.state.lock().unwrap().fail_at = Some(operation);
            self
        }

        fn with_final_appearing_after_copy(self) -> Self {
            self.state.lock().unwrap().final_after_finish_partial = true;
            self
        }

        fn record(&self, operation: &'static str) -> Result<(), TransferIoError> {
            let mut state = self.state.lock().unwrap();
            state.log.lock().unwrap().push(operation.into());
            if state.fail_at == Some(operation) {
                state.fail_at = None;
                Err(if operation == "open_source" {
                    TransferIoError::transient("connection reset while opening source")
                } else {
                    TransferIoError::permanent(format!("{operation}: disk full"))
                })
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl TransferIo<TestConnection> for FakeIo {
        type Source = Cursor<Vec<u8>>;
        type Partial = Cursor<Vec<u8>>;

        async fn open_source(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
        ) -> Result<(Self::Source, SourceFingerprint), TransferIoError> {
            self.record("open_source")?;
            let fingerprint = self.state.lock().unwrap().fingerprint.clone();
            Ok((
                Cursor::new(vec![b'x'; fingerprint.size as usize]),
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
            Ok(Cursor::new(vec![0; len as usize]))
        }

        async fn finish_source(&self, _source: &mut Self::Source) -> Result<(), TransferIoError> {
            self.record("finish_source")
        }

        async fn finish_partial(&self, partial: &mut Self::Partial) -> Result<(), TransferIoError> {
            self.record("finish_partial")?;
            let mut state = self.state.lock().unwrap();
            state.partial_len = Some(partial.get_ref().len() as u64);
            if state.final_after_finish_partial {
                state.inventory.final_exists = true;
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
            Ok(())
        }

        async fn promote_partial(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.record("promote_partial")?;
            let mut state = self.state.lock().unwrap();
            state.inventory.partial_exists = false;
            state.inventory.final_exists = true;
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
            Ok(())
        }

        async fn delete_backup(
            &self,
            _connection: &TestConnection,
            _job: &TransferJob,
            _artifacts: &ManagedArtifacts,
        ) -> Result<(), TransferIoError> {
            self.record("delete_backup")?;
            self.state.lock().unwrap().inventory.backup_exists = false;
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
            state.partial_len = None;
            Ok(())
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
            created_at_ms: 1,
            updated_at_ms: 1,
            started_at_ms: Some(1),
            finished_at_ms: None,
        }
    }

    async fn run_fake(
        connection: bool,
        io: FakeIo,
        job: TransferJob,
        control_state: super::RunnerControlState,
    ) -> (RunnerResult, Vec<String>) {
        let log = io.state.lock().unwrap().log.clone();
        let runner = SftpTransferJobRunner::with_services(
            FakeResolver {
                connection: connection.then_some(TestConnection),
                log: log.clone(),
            },
            io,
            2,
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
            .run(job, RunnerControl::new(control_rx), reporter)
            .await;
        acknowledger.await.unwrap();
        let operations = log.lock().unwrap().clone();
        (result, operations)
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
        let job = reduce_job(&job, JobEvent::Resolve(ConflictResolution::Restart), 2).unwrap();

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
