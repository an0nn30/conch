use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use russh_sftp::client::fs::File as SftpFile;
use russh_sftp::client::rawsession::Limits;
use russh_sftp::client::{RawSftpSession, SftpSession, error::Error as SftpClientError};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode, Version};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt};

use super::SourceFingerprint;
use super::copy::{ControlDecision, CopyOutcome, copy_with_checkpoint};
use super::pipelined::{ChunkSink, ChunkSource, PipelineTuning, pipelined_copy};
use super::positional::PositionalFile;
use crate::error::RemoteError;
use crate::handler::TermLabSshHandler;
use crate::sftp::open_sftp;

pub type SftpFileHandle = SftpFile;
pub type SftpSessionHandle = SftpSession;

pub async fn open_sftp_session(
    ssh: &russh::client::Handle<TermLabSshHandler>,
) -> Result<SftpSessionHandle, RemoteError> {
    open_sftp(ssh).await
}

pub fn fingerprint_local_parts(size: u64, modified: Option<SystemTime>) -> SourceFingerprint {
    SourceFingerprint {
        size,
        modified_token: modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| format!("unixNs:{}", duration.as_nanos())),
    }
}

pub fn fingerprint_remote_parts(size: u64, modified_seconds: Option<u64>) -> SourceFingerprint {
    SourceFingerprint {
        size,
        modified_token: modified_seconds.map(|seconds| format!("unixSeconds:{seconds}")),
    }
}

pub async fn fingerprint_open_local(
    path: impl AsRef<Path>,
) -> Result<(tokio::fs::File, SourceFingerprint), RemoteError> {
    let path = path.as_ref();
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RemoteError::SourceNotFound(format!("local source {} does not exist", path.display()))
        } else {
            RemoteError::Transfer(format!(
                "open local source {} failed: {error}",
                path.display()
            ))
        }
    })?;
    let metadata = file.metadata().await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RemoteError::SourceNotFound(format!("local source {} disappeared", path.display()))
        } else {
            RemoteError::Transfer(format!(
                "stat open local source {} failed: {error}",
                path.display()
            ))
        }
    })?;
    let fingerprint = fingerprint_local_parts(metadata.len(), metadata.modified().ok());
    Ok((file, fingerprint))
}

pub async fn fingerprint_open_remote(
    session: &SftpSession,
    path: &str,
) -> Result<(SftpFile, SourceFingerprint), RemoteError> {
    let mut file = session
        .open(path)
        .await
        .map_err(|error| map_remote_source_open_error(path, error))?;
    let metadata = match file.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => {
            let primary = map_remote_source_stat_error(path, error);
            let _ =
                super::close_remote_file(&mut file, "close remote source after stat failure").await;
            return Err(primary);
        }
    };
    let size = match metadata.size {
        Some(size) => size,
        None => {
            let primary =
                RemoteError::Transfer("stat open remote source did not return a size".into());
            let _ =
                super::close_remote_file(&mut file, "close remote source after unverifiable stat")
                    .await;
            return Err(primary);
        }
    };
    let fingerprint = fingerprint_remote_parts(size, metadata.mtime.map(u64::from));
    Ok((file, fingerprint))
}

fn map_remote_source_open_error(path: &str, error: SftpClientError) -> RemoteError {
    map_remote_source_error("open", path, error)
}

fn map_remote_source_stat_error(path: &str, error: SftpClientError) -> RemoteError {
    map_remote_source_error("stat open", path, error)
}

fn map_remote_source_error(operation: &str, path: &str, error: SftpClientError) -> RemoteError {
    if matches!(
        &error,
        SftpClientError::Status(status) if status.status_code == StatusCode::NoSuchFile
    ) {
        RemoteError::SourceNotFound(format!("remote source {path} does not exist"))
    } else {
        RemoteError::Transfer(format!("{operation} remote source failed: {error}"))
    }
}

pub async fn open_local_partial(
    path: impl AsRef<Path>,
    resume: bool,
) -> Result<tokio::fs::File, RemoteError> {
    tokio::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(!resume)
        .open(path.as_ref())
        .await
        .map_err(|error| {
            RemoteError::Transfer(format!(
                "open local partial {} failed: {error}",
                path.as_ref().display()
            ))
        })
}

pub async fn open_remote_partial(
    session: &SftpSession,
    path: &str,
    resume: bool,
) -> Result<SftpFile, RemoteError> {
    session
        .open_with_flags(path, remote_partial_open_flags(resume))
        .await
        .map_err(|error| RemoteError::Transfer(format!("open remote partial failed: {error}")))
}

/// Flags for a partial file that already holds bytes to resume from: create
/// if missing, open for writing, but never discard what is already there.
pub(crate) fn resume_partial_open_flags() -> OpenFlags {
    OpenFlags::CREATE | OpenFlags::WRITE
}

/// Flags for a partial file that must start empty: create if missing, open
/// for writing, and discard any stale bytes left over from a previous run.
pub(crate) fn fresh_partial_open_flags() -> OpenFlags {
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE
}

/// Single source of truth for resume-vs-fresh remote partial open flags,
/// shared by the sequential (`open_remote_partial`) and pipelined transfer
/// paths so they can never drift apart.
pub(crate) fn pipelined_remote_open_flags(resume: bool) -> OpenFlags {
    if resume { resume_partial_open_flags() } else { fresh_partial_open_flags() }
}

fn remote_partial_open_flags(resume: bool) -> OpenFlags {
    pipelined_remote_open_flags(resume)
}

fn resume_partial_from(offset: u64) -> bool {
    offset > 0
}

#[async_trait]
trait RemotePartialIo<C>: Sync {
    type RemoteDestination: AsyncWrite + AsyncSeek + Unpin + Send;
    async fn open_remote_destination(
        &self,
        connection: &C,
        path: &str,
        resume: bool,
    ) -> Result<Self::RemoteDestination, RemoteError>;
    async fn finalize_remote_destination(
        &self,
        destination: &mut Self::RemoteDestination,
        operation: &str,
    ) -> Result<(), RemoteError>;
    async fn set_remote_destination_len(
        &self,
        destination: &mut Self::RemoteDestination,
        checkpoint: u64,
    ) -> Result<(), RemoteError>;
}

#[async_trait]
trait PartialTransferIo<C>: RemotePartialIo<C> {
    type LocalSource: AsyncRead + AsyncSeek + Unpin + Send;
    type RemoteSource: AsyncRead + AsyncSeek + Unpin + Send;
    type LocalDestination: AsyncWrite + AsyncSeek + Unpin + Send;

    async fn open_local_source(
        &self,
        path: &Path,
    ) -> Result<(Self::LocalSource, SourceFingerprint), RemoteError>;
    async fn open_remote_source(
        &self,
        connection: &C,
        path: &str,
    ) -> Result<(Self::RemoteSource, SourceFingerprint), RemoteError>;
    async fn finalize_remote_source(
        &self,
        source: &mut Self::RemoteSource,
        operation: &str,
    ) -> Result<(), RemoteError>;
    async fn open_local_destination(
        &self,
        path: &Path,
        resume: bool,
    ) -> Result<Self::LocalDestination, RemoteError>;
    async fn finalize_local_destination(
        &self,
        destination: &mut Self::LocalDestination,
        operation: &str,
    ) -> Result<(), RemoteError>;
}

struct RealPartialTransferIo;

struct UploadPartialRequest<'a> {
    local_path: &'a Path,
    remote_partial_path: &'a str,
    offset: u64,
    chunk_size: usize,
}

struct DownloadPartialRequest<'a> {
    remote_path: &'a str,
    local_partial_path: &'a Path,
    offset: u64,
    chunk_size: usize,
}

#[async_trait]
impl RemotePartialIo<russh::client::Handle<TermLabSshHandler>> for RealPartialTransferIo {
    type RemoteDestination = SftpFile;

    async fn open_remote_destination(
        &self,
        connection: &russh::client::Handle<TermLabSshHandler>,
        path: &str,
        resume: bool,
    ) -> Result<Self::RemoteDestination, RemoteError> {
        let session = open_sftp(connection).await?;
        open_remote_partial(&session, path, resume).await
    }

    async fn finalize_remote_destination(
        &self,
        destination: &mut Self::RemoteDestination,
        operation: &str,
    ) -> Result<(), RemoteError> {
        super::finalize_remote_write(destination, operation).await
    }

    async fn set_remote_destination_len(
        &self,
        destination: &mut Self::RemoteDestination,
        checkpoint: u64,
    ) -> Result<(), RemoteError> {
        let mut metadata = FileAttributes::empty();
        metadata.size = Some(checkpoint);
        destination.set_metadata(metadata).await.map_err(|error| {
            RemoteError::Transfer(format!("truncate remote partial failed: {error}"))
        })
    }
}

struct RealSftpSessionPartialIo;

#[async_trait]
impl RemotePartialIo<SftpSession> for RealSftpSessionPartialIo {
    type RemoteDestination = SftpFile;

    async fn open_remote_destination(
        &self,
        session: &SftpSession,
        path: &str,
        resume: bool,
    ) -> Result<Self::RemoteDestination, RemoteError> {
        open_remote_partial(session, path, resume).await
    }

    async fn finalize_remote_destination(
        &self,
        destination: &mut Self::RemoteDestination,
        operation: &str,
    ) -> Result<(), RemoteError> {
        super::finalize_remote_write(destination, operation).await
    }

    async fn set_remote_destination_len(
        &self,
        destination: &mut Self::RemoteDestination,
        checkpoint: u64,
    ) -> Result<(), RemoteError> {
        let mut metadata = FileAttributes::empty();
        metadata.size = Some(checkpoint);
        destination.set_metadata(metadata).await.map_err(|error| {
            RemoteError::Transfer(format!("truncate remote partial failed: {error}"))
        })
    }
}

#[async_trait]
impl PartialTransferIo<russh::client::Handle<TermLabSshHandler>> for RealPartialTransferIo {
    type LocalSource = tokio::fs::File;
    type RemoteSource = SftpFile;
    type LocalDestination = tokio::fs::File;

    async fn open_local_source(
        &self,
        path: &Path,
    ) -> Result<(Self::LocalSource, SourceFingerprint), RemoteError> {
        fingerprint_open_local(path).await
    }

    async fn open_remote_source(
        &self,
        connection: &russh::client::Handle<TermLabSshHandler>,
        path: &str,
    ) -> Result<(Self::RemoteSource, SourceFingerprint), RemoteError> {
        let session = open_sftp(connection).await?;
        fingerprint_open_remote(&session, path).await
    }

    async fn finalize_remote_source(
        &self,
        source: &mut Self::RemoteSource,
        operation: &str,
    ) -> Result<(), RemoteError> {
        super::close_remote_file(source, operation).await
    }

    async fn open_local_destination(
        &self,
        path: &Path,
        resume: bool,
    ) -> Result<Self::LocalDestination, RemoteError> {
        open_local_partial(path, resume).await
    }

    async fn finalize_local_destination(
        &self,
        destination: &mut Self::LocalDestination,
        operation: &str,
    ) -> Result<(), RemoteError> {
        finalize_local_write(destination, operation).await
    }
}

pub async fn truncate_local_partial(
    path: impl AsRef<Path>,
    checkpoint: u64,
) -> Result<(), RemoteError> {
    let file = open_local_partial(path.as_ref(), true).await?;
    file.set_len(checkpoint).await.map_err(|error| {
        RemoteError::Transfer(format!(
            "truncate local partial {} failed: {error}",
            path.as_ref().display()
        ))
    })?;
    file.sync_all().await.map_err(|error| {
        RemoteError::Transfer(format!(
            "sync truncated local partial {} failed: {error}",
            path.as_ref().display()
        ))
    })
}

pub async fn truncate_remote_partial(
    session: &SftpSession,
    path: &str,
    checkpoint: u64,
) -> Result<(), RemoteError> {
    truncate_remote_partial_with_io(&RealSftpSessionPartialIo, session, path, checkpoint).await
}

async fn truncate_remote_partial_with_io<I, C>(
    io: &I,
    connection: &C,
    path: &str,
    checkpoint: u64,
) -> Result<(), RemoteError>
where
    C: Sync,
    I: RemotePartialIo<C>,
{
    let mut file = io.open_remote_destination(connection, path, true).await?;
    let truncate_result = io.set_remote_destination_len(&mut file, checkpoint).await;
    let finalize_result = io
        .finalize_remote_destination(&mut file, "truncate remote partial")
        .await;
    truncate_result.and(finalize_result)
}

async fn open_after_fingerprint<W, F, Fut, O, OFut>(
    fingerprint: SourceFingerprint,
    on_fingerprint: F,
    open_destination: O,
) -> Result<(W, u64), RemoteError>
where
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    O: FnOnce() -> OFut,
    OFut: Future<Output = Result<W, RemoteError>>,
{
    let total = fingerprint.size;
    on_fingerprint(fingerprint).await?;
    Ok((open_destination().await?, total))
}

async fn finalize_local_write(
    file: &mut tokio::fs::File,
    operation: &str,
) -> Result<(), RemoteError> {
    let flush_result = file
        .flush()
        .await
        .map_err(|error| RemoteError::Transfer(format!("{operation}: flush failed: {error}")));
    let sync_result = file
        .sync_all()
        .await
        .map_err(|error| RemoteError::Transfer(format!("{operation}: sync failed: {error}")));
    flush_result.and(sync_result)
}

fn finish_after_cleanup<T>(
    operation: Result<T, RemoteError>,
    cleanup: Result<(), RemoteError>,
) -> Result<T, RemoteError> {
    match operation {
        Ok(value) => cleanup.map(|()| value),
        Err(primary) => Err(primary),
    }
}

pub async fn upload_to_partial<F, Fut, C, P>(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    local_path: impl AsRef<Path>,
    remote_partial_path: &str,
    offset: u64,
    chunk_size: usize,
    on_fingerprint: F,
    control: C,
    progress: P,
) -> Result<CopyOutcome, RemoteError>
where
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    upload_to_partial_with_io(
        &RealPartialTransferIo,
        ssh,
        UploadPartialRequest {
            local_path: local_path.as_ref(),
            remote_partial_path,
            offset,
            chunk_size,
        },
        on_fingerprint,
        control,
        progress,
    )
    .await
}

async fn upload_to_partial_with_io<I, C, F, Fut, Control, Progress>(
    io: &I,
    connection: &C,
    request: UploadPartialRequest<'_>,
    on_fingerprint: F,
    control: Control,
    progress: Progress,
) -> Result<CopyOutcome, RemoteError>
where
    C: Sync,
    I: PartialTransferIo<C>,
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    Control: FnMut() -> ControlDecision,
    Progress: FnMut(u64, u64),
{
    let (mut source, fingerprint) = io.open_local_source(request.local_path).await?;
    let (mut destination, total) = open_after_fingerprint(fingerprint, on_fingerprint, || async {
        io.open_remote_destination(
            connection,
            request.remote_partial_path,
            resume_partial_from(request.offset),
        )
        .await
    })
    .await?;
    let copy_result = copy_with_checkpoint(
        &mut source,
        &mut destination,
        request.offset,
        total,
        request.chunk_size,
        control,
        progress,
    )
    .await;
    let finalize_result = io
        .finalize_remote_destination(&mut destination, "finish resumable upload")
        .await;
    finish_after_cleanup(copy_result, finalize_result)
}

pub async fn download_to_partial<F, Fut, C, P>(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    remote_path: &str,
    local_partial_path: impl AsRef<Path>,
    offset: u64,
    chunk_size: usize,
    on_fingerprint: F,
    control: C,
    progress: P,
) -> Result<CopyOutcome, RemoteError>
where
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    download_to_partial_with_io(
        &RealPartialTransferIo,
        ssh,
        DownloadPartialRequest {
            remote_path,
            local_partial_path: local_partial_path.as_ref(),
            offset,
            chunk_size,
        },
        on_fingerprint,
        control,
        progress,
    )
    .await
}

async fn download_to_partial_with_io<I, C, F, Fut, Control, Progress>(
    io: &I,
    connection: &C,
    request: DownloadPartialRequest<'_>,
    on_fingerprint: F,
    control: Control,
    progress: Progress,
) -> Result<CopyOutcome, RemoteError>
where
    C: Sync,
    I: PartialTransferIo<C>,
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    Control: FnMut() -> ControlDecision,
    Progress: FnMut(u64, u64),
{
    let (mut source, fingerprint) = io
        .open_remote_source(connection, request.remote_path)
        .await?;
    let (mut destination, total) = match open_after_fingerprint(fingerprint, on_fingerprint, || {
        io.open_local_destination(
            request.local_partial_path,
            resume_partial_from(request.offset),
        )
    })
    .await
    {
        Ok(opened) => opened,
        Err(primary) => {
            let _ = io
                .finalize_remote_source(
                    &mut source,
                    "close remote source after fingerprint or local open failure",
                )
                .await;
            return Err(primary);
        }
    };
    let copy_result = copy_with_checkpoint(
        &mut source,
        &mut destination,
        request.offset,
        total,
        request.chunk_size,
        control,
        progress,
    )
    .await;

    let close_result = io
        .finalize_remote_source(&mut source, "finish resumable download")
        .await;
    let local_finalize_result = io
        .finalize_local_destination(&mut destination, "finish resumable download")
        .await;
    let cleanup_result = close_result.and(local_finalize_result);
    finish_after_cleanup(copy_result, cleanup_result)
}

/// Re-stat a local source and reject if it no longer matches `expected`.
///
/// The pipelined upload path fingerprints the source once, hands the
/// fingerprint to `on_fingerprint` (which may block for an arbitrary time —
/// e.g. an overwrite confirmation), then opens a brand new [`PositionalFile`]
/// handle for the actual copy. That second open is a fresh TOCTOU window the
/// sequential path doesn't have (it keeps the original handle open across
/// the whole transfer). This closes it.
pub(crate) async fn revalidate_local_fingerprint(
    path: &Path,
    expected: &SourceFingerprint,
) -> Result<(), RemoteError> {
    let (_file, current) = fingerprint_open_local(path).await?;
    if &current != expected {
        return Err(RemoteError::Transfer(format!(
            "local source {} changed since it was checked (size {} -> {})",
            path.display(),
            expected.size,
            current.size,
        )));
    }
    Ok(())
}

/// Remote twin of [`revalidate_local_fingerprint`]. Currently unused in the
/// pipelined download path: `fingerprint_open_remote` both stats the source
/// and is the value handed to `on_fingerprint` in the same call, so there is
/// no separate reopen for it to guard against there. Kept as the symmetric
/// helper for the remote side and covered indirectly by
/// `fingerprint_open_remote`'s own tests; not unit-testable on its own
/// without a live SFTP session, same as `fingerprint_open_remote`.
#[allow(dead_code)]
pub(crate) async fn revalidate_remote_fingerprint(
    sftp: &SftpSessionHandle,
    path: &str,
    expected: &SourceFingerprint,
) -> Result<(), RemoteError> {
    let (mut file, current) = fingerprint_open_remote(sftp, path).await?;
    let mismatch = if &current != expected {
        Some(RemoteError::Transfer(format!(
            "remote source {path} changed since it was checked (size {} -> {})",
            expected.size, current.size,
        )))
    } else {
        None
    };
    let close_result =
        super::close_remote_file(&mut file, "close remote source after revalidation").await;
    match mismatch {
        Some(error) => Err(error),
        None => close_result,
    }
}

/// OpenSSH's conventional read/write length cap (see
/// `russh_sftp::client::fs::file::MAX_READ_LENGTH` / `MAX_WRITE_LENGTH`,
/// 261120 bytes = 255 KiB). `RawSftpSession` negotiates and self-enforces the
/// `limits@openssh.com` extension internally (`limits()` / `set_limits`) but
/// keeps the negotiated numbers private — there is no accessor to read them
/// back — so pipelined chunk sizing clamps to this safe default instead of
/// the server's exact advertised limit.
const RAW_SFTP_MAX_CHUNK_BYTES: usize = 255 * 1024;

fn clamp_pipelined_chunk_bytes(tuning: PipelineTuning) -> PipelineTuning {
    PipelineTuning {
        depth: tuning.depth,
        chunk_bytes: tuning.chunk_bytes.min(RAW_SFTP_MAX_CHUNK_BYTES),
    }
}

/// Negotiate the `limits@openssh.com` extension the same way
/// `SftpSession::new_opts` does (see russh-sftp's `client/session.rs`), so
/// the raw session self-enforces the server's advertised read/write caps.
async fn negotiate_raw_sftp_limits(raw: &mut RawSftpSession, version: &Version) {
    if version
        .extensions
        .get(russh_sftp::extensions::LIMITS)
        .is_some_and(|value| value == "1")
        && let Ok(limits) = raw.limits().await
    {
        raw.set_limits(Arc::new(Limits::from(limits)));
    }
}

/// Open a second SFTP channel on the already-authenticated SSH handle and
/// hand back the raw protocol session (mirrors `crate::sftp::open_sftp`, but
/// returns the low-level `RawSftpSession` instead of the high-level
/// `SftpSession` wrapper) so the pipelined engine can issue concurrent
/// offset-addressed reads/writes against one handle instead of the
/// sequential, cursor-based `File` API.
pub(crate) async fn open_raw_sftp_session(
    ssh: &russh::client::Handle<TermLabSshHandler>,
) -> Result<RawSftpSession, RemoteError> {
    let channel = ssh.channel_open_session().await.map_err(|error| {
        RemoteError::Transfer(format!("open pipelined channel failed: {error}"))
    })?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| {
            RemoteError::Transfer(format!("request sftp subsystem failed: {error}"))
        })?;
    let mut raw = RawSftpSession::new(channel.into_stream());
    let version = raw
        .init()
        .await
        .map_err(|error| RemoteError::Transfer(format!("sftp init failed: {error}")))?;
    negotiate_raw_sftp_limits(&mut raw, &version).await;
    Ok(raw)
}

/// A remote file accessed through a [`RawSftpSession`] handle for the
/// pipelined engine: unlike `russh_sftp::client::fs::File`, the raw
/// `read`/`write` calls are offset-addressed and hold no internal cursor, so
/// concurrent chunk tasks can share one handle safely.
struct RawRemoteChunkFile {
    session: RawSftpSession,
    handle: String,
}

#[async_trait]
impl ChunkSource for RawRemoteChunkFile {
    async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, std::io::Error> {
        match self.session.read(self.handle.as_str(), offset, len as u32).await {
            Ok(data) => Ok(data.data),
            // The engine's short-read/tail logic owns EOF handling; an SFTP
            // EOF status is just an empty read, not a transport failure.
            Err(SftpClientError::Status(status)) if status.status_code == StatusCode::Eof => {
                Ok(Vec::new())
            }
            Err(error) => Err(std::io::Error::other(error.to_string())),
        }
    }
}

#[async_trait]
impl ChunkSink for RawRemoteChunkFile {
    async fn write_at(&self, offset: u64, data: Vec<u8>) -> Result<(), std::io::Error> {
        self.session
            .write(self.handle.as_str(), offset, data)
            .await
            .map(|_| ())
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

#[async_trait]
impl ChunkSource for PositionalFile {
    async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, std::io::Error> {
        self.read_at(offset, len).await
    }
}

#[async_trait]
impl ChunkSink for PositionalFile {
    async fn write_at(&self, offset: u64, data: Vec<u8>) -> Result<(), std::io::Error> {
        self.write_at(offset, data).await
    }
}

/// Pipelined twin of [`upload_to_partial`]: same fingerprint ->
/// `on_fingerprint` -> open destination -> copy order, but drives the copy
/// through [`pipelined_copy`] over a dedicated raw SFTP handle instead of
/// `copy_with_checkpoint`'s single sequential stream.
pub async fn upload_to_partial_pipelined<F, Fut, C, P>(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    local_path: impl AsRef<Path>,
    remote_partial_path: &str,
    offset: u64,
    tuning: PipelineTuning,
    on_fingerprint: F,
    control: C,
    progress: P,
) -> Result<CopyOutcome, RemoteError>
where
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    let local_path = local_path.as_ref();
    let (_local_stat_handle, fingerprint) = fingerprint_open_local(local_path).await?;
    let total = fingerprint.size;
    on_fingerprint(fingerprint.clone()).await?;
    // Close the reopen TOCTOU window: `on_fingerprint` may have taken an
    // arbitrary amount of time before we open a fresh handle below.
    revalidate_local_fingerprint(local_path, &fingerprint).await?;

    let source = PositionalFile::open_read(local_path).map_err(|error| {
        RemoteError::Transfer(format!(
            "open local source {} for pipelined upload failed: {error}",
            local_path.display()
        ))
    })?;

    let raw = open_raw_sftp_session(ssh).await?;
    let opened = raw
        .open(
            remote_partial_path,
            pipelined_remote_open_flags(offset > 0),
            FileAttributes::empty(),
        )
        .await
        .map_err(|error| {
            RemoteError::Transfer(format!(
                "open remote partial {remote_partial_path} failed: {error}"
            ))
        })?;
    let sink = RawRemoteChunkFile { session: raw, handle: opened.handle };

    let tuning = clamp_pipelined_chunk_bytes(tuning);
    let copy_result = pipelined_copy(&source, &sink, offset, total, tuning, control, progress)
        .await
        .map_err(RemoteError::from);
    let close_result = sink
        .session
        .close(sink.handle.as_str())
        .await
        .map(|_| ())
        .map_err(|error| RemoteError::Transfer(format!("close remote partial failed: {error}")));

    finish_after_cleanup(copy_result, close_result)
}

/// Pipelined twin of [`download_to_partial`]: same fingerprint ->
/// `on_fingerprint` -> open destination -> copy -> sync order, but drives
/// the copy through [`pipelined_copy`] over a dedicated raw SFTP handle.
pub async fn download_to_partial_pipelined<F, Fut, C, P>(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    remote_path: &str,
    local_partial_path: impl AsRef<Path>,
    offset: u64,
    tuning: PipelineTuning,
    on_fingerprint: F,
    control: C,
    progress: P,
) -> Result<CopyOutcome, RemoteError>
where
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    let local_partial_path = local_partial_path.as_ref();

    let sftp_session = open_sftp_session(ssh).await?;
    let (mut fingerprint_handle, fingerprint) =
        fingerprint_open_remote(&sftp_session, remote_path).await?;
    let total = fingerprint.size;
    // Mirrors the sequential path's `open_after_fingerprint` handling: if
    // `on_fingerprint` rejects (e.g. a queued overwrite decision), the
    // fingerprint handle must still be closed rather than left dangling.
    if let Err(primary) = on_fingerprint(fingerprint).await {
        let _ = super::close_remote_file(
            &mut fingerprint_handle,
            "close remote source after pipelined fingerprint rejection",
        )
        .await;
        return Err(primary);
    }
    super::close_remote_file(
        &mut fingerprint_handle,
        "close remote source after pipelined fingerprint",
    )
    .await?;

    let raw = open_raw_sftp_session(ssh).await?;
    let opened = raw
        .open(remote_path, OpenFlags::READ, FileAttributes::empty())
        .await
        .map_err(|error| {
            RemoteError::Transfer(format!(
                "open remote source {remote_path} for pipelined download failed: {error}"
            ))
        })?;
    let source = RawRemoteChunkFile { session: raw, handle: opened.handle };

    let sink = PositionalFile::open_write(local_partial_path, offset == 0).map_err(|error| {
        RemoteError::Transfer(format!(
            "open local partial {} for pipelined download failed: {error}",
            local_partial_path.display()
        ))
    })?;

    let tuning = clamp_pipelined_chunk_bytes(tuning);
    let copy_result = pipelined_copy(&source, &sink, offset, total, tuning, control, progress)
        .await
        .map_err(RemoteError::from);

    let close_result = source
        .session
        .close(source.handle.as_str())
        .await
        .map(|_| ())
        .map_err(|error| RemoteError::Transfer(format!("close remote source failed: {error}")));
    let sync_result = sink.sync().await.map_err(|error| {
        RemoteError::Transfer(format!(
            "sync local partial {} failed: {error}",
            local_partial_path.display()
        ))
    });

    finish_after_cleanup(copy_result, close_result.and(sync_result))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, UNIX_EPOCH};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        DownloadPartialRequest, PartialTransferIo, RemotePartialIo, UploadPartialRequest,
        download_to_partial_with_io, fingerprint_local_parts, fingerprint_open_local,
        fingerprint_remote_parts, fresh_partial_open_flags, map_remote_source_open_error,
        open_after_fingerprint, open_local_partial, pipelined_remote_open_flags,
        remote_partial_open_flags, resume_partial_from, resume_partial_open_flags,
        revalidate_local_fingerprint, truncate_local_partial, truncate_remote_partial_with_io,
        upload_to_partial_with_io,
    };
    use crate::error::RemoteError;
    use russh_sftp::protocol::OpenFlags;
    use russh_sftp::{
        client::error::Error as SftpClientError,
        protocol::{Status, StatusCode},
    };

    #[derive(Clone)]
    struct FakePartialTransferIo {
        events: Arc<Mutex<Vec<String>>>,
        local_source: Vec<u8>,
        remote_source: Vec<u8>,
        remote_partial: Arc<Mutex<Vec<u8>>>,
        local_partial: Arc<Mutex<Vec<u8>>>,
    }

    impl FakePartialTransferIo {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                local_source: b"abcdef".to_vec(),
                remote_source: b"uvwxyz".to_vec(),
                remote_partial: Arc::new(Mutex::new(b"abcOLD".to_vec())),
                local_partial: Arc::new(Mutex::new(b"uvwOLD".to_vec())),
            }
        }

        fn record(&self, event: impl Into<String>) {
            self.events.lock().unwrap().push(event.into());
        }
    }

    #[tokio::test]
    async fn missing_local_source_has_typed_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-source.bin");

        let error = fingerprint_open_local(&missing).await.unwrap_err();

        assert!(matches!(
            error,
            crate::error::RemoteError::SourceNotFound(message)
                if message.contains("missing-source.bin")
        ));
    }

    #[test]
    fn remote_no_such_file_status_has_typed_provenance() {
        let error = map_remote_source_open_error(
            "/srv/missing-source.bin",
            SftpClientError::Status(Status {
                id: 1,
                status_code: StatusCode::NoSuchFile,
                error_message: "not present".into(),
                language_tag: "en".into(),
            }),
        );

        assert!(matches!(
            error,
            crate::error::RemoteError::SourceNotFound(message)
                if message.contains("/srv/missing-source.bin")
        ));
    }

    #[async_trait::async_trait]
    impl RemotePartialIo<()> for FakePartialTransferIo {
        type RemoteDestination = Cursor<Vec<u8>>;

        async fn open_remote_destination(
            &self,
            _connection: &(),
            _path: &str,
            resume: bool,
        ) -> Result<Self::RemoteDestination, crate::error::RemoteError> {
            self.record(format!("open-remote:{resume}"));
            let bytes = if resume {
                self.remote_partial.lock().unwrap().clone()
            } else {
                Vec::new()
            };
            Ok(Cursor::new(bytes))
        }

        async fn finalize_remote_destination(
            &self,
            destination: &mut Self::RemoteDestination,
            _operation: &str,
        ) -> Result<(), crate::error::RemoteError> {
            self.record("finalize-remote");
            *self.remote_partial.lock().unwrap() = destination.get_ref().clone();
            Ok(())
        }

        async fn set_remote_destination_len(
            &self,
            destination: &mut Self::RemoteDestination,
            checkpoint: u64,
        ) -> Result<(), crate::error::RemoteError> {
            self.record(format!("set-remote-len:{checkpoint}"));
            destination.get_mut().resize(checkpoint as usize, 0);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl PartialTransferIo<()> for FakePartialTransferIo {
        type LocalSource = Cursor<Vec<u8>>;
        type RemoteSource = Cursor<Vec<u8>>;
        type LocalDestination = Cursor<Vec<u8>>;

        async fn open_local_source(
            &self,
            _path: &Path,
        ) -> Result<(Self::LocalSource, super::SourceFingerprint), crate::error::RemoteError>
        {
            self.record("open-local-source");
            Ok((
                Cursor::new(self.local_source.clone()),
                fingerprint_local_parts(self.local_source.len() as u64, None),
            ))
        }

        async fn open_remote_source(
            &self,
            _connection: &(),
            _path: &str,
        ) -> Result<(Self::RemoteSource, super::SourceFingerprint), crate::error::RemoteError>
        {
            self.record("open-remote-source");
            Ok((
                Cursor::new(self.remote_source.clone()),
                fingerprint_remote_parts(self.remote_source.len() as u64, Some(7)),
            ))
        }

        async fn finalize_remote_source(
            &self,
            _source: &mut Self::RemoteSource,
            _operation: &str,
        ) -> Result<(), crate::error::RemoteError> {
            self.record("finalize-remote-source");
            Ok(())
        }

        async fn open_local_destination(
            &self,
            _path: &Path,
            resume: bool,
        ) -> Result<Self::LocalDestination, crate::error::RemoteError> {
            self.record(format!("open-local:{resume}"));
            let bytes = if resume {
                self.local_partial.lock().unwrap().clone()
            } else {
                Vec::new()
            };
            Ok(Cursor::new(bytes))
        }

        async fn finalize_local_destination(
            &self,
            destination: &mut Self::LocalDestination,
            _operation: &str,
        ) -> Result<(), crate::error::RemoteError> {
            self.record("finalize-local");
            *self.local_partial.lock().unwrap() = destination.get_ref().clone();
            Ok(())
        }
    }

    #[test]
    fn fresh_and_resumed_wrapper_offsets_select_safe_partial_open_flags() {
        assert!(!resume_partial_from(0));
        assert!(resume_partial_from(1));

        let fresh = remote_partial_open_flags(resume_partial_from(0));
        assert!(fresh.contains(OpenFlags::CREATE));
        assert!(fresh.contains(OpenFlags::WRITE));
        assert!(fresh.contains(OpenFlags::TRUNCATE));

        let resumed = remote_partial_open_flags(resume_partial_from(4_096));
        assert!(resumed.contains(OpenFlags::CREATE));
        assert!(resumed.contains(OpenFlags::WRITE));
        assert!(!resumed.contains(OpenFlags::TRUNCATE));
    }

    #[tokio::test]
    async fn upload_wrapper_wiring_uses_offset_to_select_resume_without_truncation() {
        let io = FakePartialTransferIo::new();
        let fingerprint_events = Arc::clone(&io.events);

        let outcome = upload_to_partial_with_io(
            &io,
            &(),
            UploadPartialRequest {
                local_path: Path::new("source.bin"),
                remote_partial_path: "/partial.bin",
                offset: 3,
                chunk_size: 2,
            },
            move |_| async move {
                fingerprint_events
                    .lock()
                    .unwrap()
                    .push("fingerprint".into());
                Ok(())
            },
            || super::ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            super::CopyOutcome::Completed { bytes: 6 }
        ));
        assert_eq!(*io.remote_partial.lock().unwrap(), b"abcdef");
        assert_eq!(
            *io.events.lock().unwrap(),
            vec![
                "open-local-source",
                "fingerprint",
                "open-remote:true",
                "finalize-remote",
            ]
        );
    }

    #[tokio::test]
    async fn upload_wrapper_wiring_truncates_a_fresh_partial() {
        let io = FakePartialTransferIo::new();

        upload_to_partial_with_io(
            &io,
            &(),
            UploadPartialRequest {
                local_path: Path::new("source.bin"),
                remote_partial_path: "/partial.bin",
                offset: 0,
                chunk_size: 3,
            },
            |_| async { Ok(()) },
            || super::ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(*io.remote_partial.lock().unwrap(), b"abcdef");
        assert!(
            io.events
                .lock()
                .unwrap()
                .iter()
                .any(|event| event == "open-remote:false")
        );
    }

    #[tokio::test]
    async fn download_wrapper_wiring_uses_offset_to_preserve_the_partial_prefix() {
        let io = FakePartialTransferIo::new();

        let outcome = download_to_partial_with_io(
            &io,
            &(),
            DownloadPartialRequest {
                remote_path: "/source.bin",
                local_partial_path: Path::new("partial.bin"),
                offset: 3,
                chunk_size: 2,
            },
            |_| async { Ok(()) },
            || super::ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            super::CopyOutcome::Completed { bytes: 6 }
        ));
        assert_eq!(*io.local_partial.lock().unwrap(), b"uvwxyz");
        assert_eq!(
            *io.events.lock().unwrap(),
            vec![
                "open-remote-source",
                "open-local:true",
                "finalize-remote-source",
                "finalize-local",
            ]
        );
    }

    #[tokio::test]
    async fn remote_truncate_wrapper_opens_in_resume_mode_and_sets_the_checkpoint() {
        let io = FakePartialTransferIo::new();

        truncate_remote_partial_with_io(&io, &(), "/partial.bin", 4)
            .await
            .unwrap();

        assert_eq!(*io.remote_partial.lock().unwrap(), b"abcO");
        assert_eq!(
            *io.events.lock().unwrap(),
            vec!["open-remote:true", "set-remote-len:4", "finalize-remote",]
        );
    }

    #[test]
    fn local_fingerprints_preserve_nanosecond_mtime_precision() {
        let first =
            fingerprint_local_parts(7, Some(UNIX_EPOCH + Duration::new(1_700_000_000, 123)));
        let second =
            fingerprint_local_parts(7, Some(UNIX_EPOCH + Duration::new(1_700_000_000, 124)));

        assert_eq!(first.size, second.size);
        assert_eq!(
            first.modified_token.as_deref(),
            Some("unixNs:1700000000000000123")
        );
        assert_eq!(
            second.modified_token.as_deref(),
            Some("unixNs:1700000000000000124")
        );
        assert_ne!(first.modified_token, second.modified_token);
    }

    #[test]
    fn remote_fingerprints_use_second_tokens_and_keep_missing_mtime_unverifiable() {
        let verifiable = fingerprint_remote_parts(9, Some(1_700_000_000));
        let unverifiable = fingerprint_remote_parts(9, None);

        assert_eq!(
            verifiable.modified_token.as_deref(),
            Some("unixSeconds:1700000000")
        );
        assert_eq!(unverifiable.modified_token, None);
    }

    #[tokio::test]
    async fn fingerprint_and_bytes_come_from_the_same_open_local_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let replacement = directory.path().join("replacement.txt");
        tokio::fs::write(&source, b"old").await.unwrap();

        let (mut opened, fingerprint) = fingerprint_open_local(&source).await.unwrap();
        tokio::fs::write(&replacement, b"newer-content")
            .await
            .unwrap();
        tokio::fs::rename(&replacement, &source).await.unwrap();

        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"old");
        assert_eq!(fingerprint.size, 3);
        assert!(fingerprint.modified_token.is_some());
        assert_eq!(tokio::fs::read(&source).await.unwrap(), b"newer-content");
    }

    #[tokio::test]
    async fn resume_open_preserves_existing_partial_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let partial = directory.path().join("download.partial");
        tokio::fs::write(&partial, b"0123456789").await.unwrap();

        let mut opened = open_local_partial(&partial, true).await.unwrap();
        opened.write_all(b"xx").await.unwrap();
        opened.flush().await.unwrap();

        assert_eq!(tokio::fs::read(&partial).await.unwrap(), b"xx23456789");
    }

    #[tokio::test]
    async fn explicit_reconciliation_truncates_to_the_durable_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let partial = directory.path().join("download.partial");
        tokio::fs::write(&partial, b"0123456789").await.unwrap();

        truncate_local_partial(&partial, 4).await.unwrap();

        assert_eq!(tokio::fs::read(&partial).await.unwrap(), b"0123");
    }

    #[tokio::test]
    async fn fresh_open_starts_with_an_empty_partial() {
        let directory = tempfile::tempdir().unwrap();
        let partial = directory.path().join("download.partial");
        tokio::fs::write(&partial, b"stale").await.unwrap();

        drop(open_local_partial(&partial, false).await.unwrap());

        assert_eq!(tokio::fs::metadata(&partial).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn fingerprint_acknowledgement_precedes_partial_open() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&events);
        let open_events = Arc::clone(&events);

        let (destination, total) = open_after_fingerprint(
            fingerprint_remote_parts(5, Some(42)),
            move |_| async move {
                callback_events.lock().unwrap().push("fingerprint");
                Ok(())
            },
            move || async move {
                open_events.lock().unwrap().push("open");
                Ok("destination")
            },
        )
        .await
        .unwrap();

        assert_eq!(destination, "destination");
        assert_eq!(total, 5);
        assert_eq!(*events.lock().unwrap(), vec!["fingerprint", "open"]);
    }

    #[tokio::test]
    async fn rejected_fingerprint_acknowledgement_prevents_partial_open() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let open_events = Arc::clone(&events);

        let result = open_after_fingerprint(
            fingerprint_remote_parts(5, Some(41)),
            |_| async {
                Err(crate::error::RemoteError::Transfer(
                    "restart or skip required".into(),
                ))
            },
            move || async move {
                open_events.lock().unwrap().push("open");
                Ok("destination")
            },
        )
        .await;

        assert!(
            matches!(result, Err(crate::error::RemoteError::Transfer(message)) if message == "restart or skip required")
        );
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn pipelined_open_flags_match_sequential_partial_flags() {
        // `OpenFlags` (russh-sftp) doesn't implement `PartialEq`; compare the
        // underlying bits instead.
        assert_eq!(
            pipelined_remote_open_flags(true).bits(),
            resume_partial_open_flags().bits(),
            "resume must not truncate the partial",
        );
        assert_eq!(
            pipelined_remote_open_flags(false).bits(),
            fresh_partial_open_flags().bits(),
            "fresh transfers must truncate",
        );
        assert_eq!(
            pipelined_remote_open_flags(true).bits(),
            remote_partial_open_flags(true).bits(),
            "open_remote_partial must share the same resume flags",
        );
        assert_eq!(
            pipelined_remote_open_flags(false).bits(),
            remote_partial_open_flags(false).bits(),
            "open_remote_partial must share the same fresh flags",
        );
    }

    #[tokio::test]
    async fn pipelined_upload_rejects_source_changed_since_fingerprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("source.bin");
        std::fs::write(&path, b"original").expect("seed");
        let stale = fingerprint_local_parts(999, None); // wrong size on purpose
        let error = revalidate_local_fingerprint(&path, &stale)
            .await
            .expect_err("changed source must be rejected");
        assert!(matches!(error, RemoteError::Transfer(_)));
        let live = fingerprint_open_local(&path).await.expect("fingerprint").1;
        revalidate_local_fingerprint(&path, &live)
            .await
            .expect("matching fingerprint passes");
    }
}
