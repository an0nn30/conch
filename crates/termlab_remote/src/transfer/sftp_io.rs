use std::future::Future;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use russh_sftp::client::SftpSession;
use russh_sftp::client::fs::File as SftpFile;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::AsyncWriteExt;

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
    let file = tokio::fs::File::open(path.as_ref())
        .await
        .map_err(|error| {
            RemoteError::Transfer(format!(
                "open local source {} failed: {error}",
                path.as_ref().display()
            ))
        })?;
    let metadata = file.metadata().await.map_err(|error| {
        RemoteError::Transfer(format!(
            "stat open local source {} failed: {error}",
            path.as_ref().display()
        ))
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
        .map_err(|error| RemoteError::Transfer(format!("open remote source failed: {error}")))?;
    let metadata = match file.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => {
            let primary = RemoteError::Transfer(format!("stat open remote source failed: {error}"));
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
    let mut flags = OpenFlags::CREATE | OpenFlags::WRITE;
    if !resume {
        flags |= OpenFlags::TRUNCATE;
    }
    session
        .open_with_flags(path, flags)
        .await
        .map_err(|error| RemoteError::Transfer(format!("open remote partial failed: {error}")))
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
    let mut file = open_remote_partial(session, path, true).await?;
    let mut metadata = FileAttributes::empty();
    metadata.size = Some(checkpoint);
    let truncate_result = file
        .set_metadata(metadata)
        .await
        .map_err(|error| RemoteError::Transfer(format!("truncate remote partial failed: {error}")));
    let finalize_result = super::finalize_remote_write(&mut file, "truncate remote partial").await;

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
    let (mut source, fingerprint) = fingerprint_open_local(local_path).await?;
    let (mut destination, total) = open_after_fingerprint(fingerprint, on_fingerprint, || async {
        let session = open_sftp(ssh).await?;
        open_remote_partial(&session, remote_partial_path, offset > 0).await
    })
    .await?;
    let copy_result = copy_with_checkpoint(
        &mut source,
        &mut destination,
        offset,
        total,
        chunk_size,
        control,
        progress,
    )
    .await;
    let finalize_result =
        super::finalize_remote_write(&mut destination, "finish resumable upload").await;
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
    let session = open_sftp(ssh).await?;
    let (mut source, fingerprint) = fingerprint_open_remote(&session, remote_path).await?;
    let local_partial_path = local_partial_path.as_ref();
    let (mut destination, total) = match open_after_fingerprint(fingerprint, on_fingerprint, || {
        open_local_partial(local_partial_path, offset > 0)
    })
    .await
    {
        Ok(opened) => opened,
        Err(primary) => {
            let _ = super::close_remote_file(
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
        offset,
        total,
        chunk_size,
        control,
        progress,
    )
    .await;

    let close_result = super::close_remote_file(&mut source, "finish resumable download").await;
    let local_finalize_result =
        finalize_local_write(&mut destination, "finish resumable download").await;
    let cleanup_result = close_result.and(local_finalize_result);
    finish_after_cleanup(copy_result, cleanup_result)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, UNIX_EPOCH};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        fingerprint_local_parts, fingerprint_open_local, fingerprint_remote_parts,
        open_after_fingerprint, open_local_partial, truncate_local_partial,
    };

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
