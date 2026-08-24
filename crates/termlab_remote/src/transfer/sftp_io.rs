use std::future::Future;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use russh_sftp::client::fs::File as SftpFile;
use russh_sftp::client::{SftpSession, error::Error as SftpClientError};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt};

use super::SourceFingerprint;
use super::copy::{ControlDecision, CopyOutcome, copy_with_checkpoint};
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

fn remote_partial_open_flags(resume: bool) -> OpenFlags {
    let mut flags = OpenFlags::CREATE | OpenFlags::WRITE;
    if !resume {
        flags |= OpenFlags::TRUNCATE;
    }
    flags
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
        fingerprint_remote_parts, map_remote_source_open_error, open_after_fingerprint,
        open_local_partial, remote_partial_open_flags, resume_partial_from, truncate_local_partial,
        truncate_remote_partial_with_io, upload_to_partial_with_io,
    };
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
}
