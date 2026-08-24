//! File transfer engine — upload/download with progress events.
//!
//! Uses SFTP for transfers. Future: rsync detection and fallback.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;
use ts_rs::TS;

use crate::error::RemoteError;
use crate::handler::TermLabSshHandler;
use crate::sftp;
use crate::sftp::open_sftp;

mod fingerprint;
pub use fingerprint::SourceFingerprint;

/// Flush and close a remote file before the transfer is reported complete.
///
/// `russh-sftp::client::File` documents that callers must explicitly shut the
/// handle down. Dropping it only schedules a close in the background, which
/// can let the completion event and directory refresh race ahead of the
/// server making the new contents visible.
async fn finalize_remote_write<W>(file: &mut W, operation: &str) -> Result<(), RemoteError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    file.flush()
        .await
        .map_err(|e| RemoteError::Transfer(format!("{operation}: flush failed: {e}")))?;
    file.shutdown()
        .await
        .map_err(|e| RemoteError::Transfer(format!("{operation}: close failed: {e}")))?;
    Ok(())
}

async fn close_remote_file<W>(file: &mut W, operation: &str) -> Result<(), RemoteError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    file.shutdown()
        .await
        .map_err(|e| RemoteError::Transfer(format!("{operation}: close failed: {e}")))
}

// ---------------------------------------------------------------------------
// Transfer types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    Download,
    Upload,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct TransferProgress {
    pub transfer_id: String,
    pub kind: TransferKind,
    pub status: TransferStatus,
    #[ts(as = "f64")]
    pub bytes_transferred: u64,
    #[ts(as = "f64")]
    pub total_bytes: u64,
    pub file_name: String,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Transfer handle
// ---------------------------------------------------------------------------

pub struct TransferHandle {
    pub cancelled: Arc<AtomicBool>,
    pub abort_handle: tokio::task::AbortHandle,
}

// ---------------------------------------------------------------------------
// Transfer registry
// ---------------------------------------------------------------------------

pub struct TransferRegistry {
    pub transfers: HashMap<String, TransferHandle>,
}

impl TransferRegistry {
    pub fn new() -> Self {
        Self {
            transfers: HashMap::new(),
        }
    }

    pub fn cancel(&mut self, transfer_id: &str) -> bool {
        if let Some(handle) = self.transfers.remove(transfer_id) {
            handle.cancelled.store(true, Ordering::Relaxed);
            handle.abort_handle.abort();
            true
        } else {
            false
        }
    }

    pub fn cleanup_finished(&mut self) {
        self.transfers.retain(|_, h| !h.abort_handle.is_finished());
    }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Start a background download from remote to local.
/// Returns the transfer_id.
pub fn start_download(
    transfer_id: String,
    ssh_handle: Arc<russh::client::Handle<TermLabSshHandler>>,
    remote_path: String,
    local_path: String,
    progress_tx: mpsc::UnboundedSender<TransferProgress>,
    registry: Arc<Mutex<TransferRegistry>>,
) -> String {
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = Arc::clone(&cancelled);
    let tid = transfer_id.clone();
    let registry_for_task = Arc::clone(&registry);

    let task = tokio::spawn(async move {
        let file_name = remote_path
            .rsplit('/')
            .next()
            .unwrap_or(&remote_path)
            .to_string();

        let _ = progress_tx.send(TransferProgress {
            transfer_id: tid.clone(),
            kind: TransferKind::Download,
            status: TransferStatus::InProgress,
            bytes_transferred: 0,
            total_bytes: 0,
            file_name: file_name.clone(),
            error: None,
        });

        let result = download_file(
            &ssh_handle,
            &remote_path,
            &local_path,
            &cancelled_clone,
            &tid,
            &file_name,
            &progress_tx,
        )
        .await
        .map_err(|e| e.to_string());

        let status = if cancelled_clone.load(Ordering::Relaxed) {
            TransferStatus::Cancelled
        } else if result.is_ok() {
            TransferStatus::Completed
        } else {
            TransferStatus::Failed
        };

        let _ = progress_tx.send(TransferProgress {
            transfer_id: tid.clone(),
            kind: TransferKind::Download,
            status,
            bytes_transferred: result.as_ref().copied().unwrap_or(0),
            total_bytes: result.as_ref().copied().unwrap_or(0),
            file_name,
            error: result.err(),
        });

        registry_for_task.lock().transfers.remove(&tid);
    });

    let handle = TransferHandle {
        cancelled,
        abort_handle: task.abort_handle(),
    };
    registry
        .lock()
        .transfers
        .insert(transfer_id.clone(), handle);

    transfer_id
}

/// Download a single file via SFTP in chunks.
async fn download_file(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    remote_path: &str,
    local_path: &str,
    cancelled: &AtomicBool,
    transfer_id: &str,
    file_name: &str,
    progress_tx: &mpsc::UnboundedSender<TransferProgress>,
) -> Result<u64, RemoteError> {
    use std::time::Instant;
    use tokio::io::AsyncReadExt;

    let stat = sftp::stat(ssh, remote_path).await?;
    let total_bytes = stat.size;

    let sftp_session = open_sftp(ssh).await?;
    let mut remote_file = sftp_session
        .open(remote_path)
        .await
        .map_err(|e| RemoteError::Transfer(format!("open failed: {e}")))?;

    let mut local_file = std::fs::File::create(local_path)?;

    let mut bytes_transferred: u64 = 0;
    let chunk_size = 256 * 1024;
    let mut buf = vec![0u8; chunk_size];
    let mut last_progress = Instant::now();
    let progress_interval = std::time::Duration::from_millis(100);

    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RemoteError::Transfer("Transfer cancelled".into()));
        }

        let n = remote_file
            .read(&mut buf)
            .await
            .map_err(|e| RemoteError::Transfer(format!("read failed: {e}")))?;

        if n == 0 {
            break;
        }

        std::io::Write::write_all(&mut local_file, &buf[..n])?;

        bytes_transferred += n as u64;

        // Throttle progress events to avoid flooding the frontend.
        if last_progress.elapsed() >= progress_interval {
            last_progress = Instant::now();
            let _ = progress_tx.send(TransferProgress {
                transfer_id: transfer_id.to_string(),
                kind: TransferKind::Download,
                status: TransferStatus::InProgress,
                bytes_transferred,
                total_bytes,
                file_name: file_name.to_string(),
                error: None,
            });
        }
    }

    close_remote_file(&mut remote_file, "finish remote download").await?;
    std::io::Write::flush(&mut local_file)?;

    Ok(bytes_transferred)
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// Start a background upload from local to remote.
/// Returns the transfer_id.
pub fn start_upload(
    transfer_id: String,
    ssh_handle: Arc<russh::client::Handle<TermLabSshHandler>>,
    local_path: String,
    remote_path: String,
    progress_tx: mpsc::UnboundedSender<TransferProgress>,
    registry: Arc<Mutex<TransferRegistry>>,
) -> String {
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = Arc::clone(&cancelled);
    let tid = transfer_id.clone();
    let registry_for_task = Arc::clone(&registry);

    let task = tokio::spawn(async move {
        let file_name = local_path
            .rsplit('/')
            .next()
            .unwrap_or(&local_path)
            .to_string();

        let _ = progress_tx.send(TransferProgress {
            transfer_id: tid.clone(),
            kind: TransferKind::Upload,
            status: TransferStatus::InProgress,
            bytes_transferred: 0,
            total_bytes: 0,
            file_name: file_name.clone(),
            error: None,
        });

        let result = upload_file(
            &ssh_handle,
            &local_path,
            &remote_path,
            &cancelled_clone,
            &tid,
            &file_name,
            &progress_tx,
        )
        .await
        .map_err(|e| e.to_string());

        let status = if cancelled_clone.load(Ordering::Relaxed) {
            TransferStatus::Cancelled
        } else if result.is_ok() {
            TransferStatus::Completed
        } else {
            TransferStatus::Failed
        };

        let _ = progress_tx.send(TransferProgress {
            transfer_id: tid.clone(),
            kind: TransferKind::Upload,
            status,
            bytes_transferred: result.as_ref().copied().unwrap_or(0),
            total_bytes: result.as_ref().copied().unwrap_or(0),
            file_name,
            error: result.err(),
        });

        registry_for_task.lock().transfers.remove(&tid);
    });

    let handle = TransferHandle {
        cancelled,
        abort_handle: task.abort_handle(),
    };
    registry
        .lock()
        .transfers
        .insert(transfer_id.clone(), handle);

    transfer_id
}

/// Upload a single file via SFTP in chunks.
async fn upload_file(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    local_path: &str,
    remote_path: &str,
    cancelled: &AtomicBool,
    transfer_id: &str,
    file_name: &str,
    progress_tx: &mpsc::UnboundedSender<TransferProgress>,
) -> Result<u64, RemoteError> {
    use std::time::Instant;
    use tokio::io::AsyncWriteExt;

    let local_meta = std::fs::metadata(local_path)?;
    let total_bytes = local_meta.len();

    let mut local_file = std::fs::File::open(local_path)?;

    let sftp_session = open_sftp(ssh).await?;
    let mut remote_file = sftp_session
        .create(remote_path)
        .await
        .map_err(|e| RemoteError::Transfer(format!("create remote file: {e}")))?;

    let mut bytes_transferred: u64 = 0;
    let chunk_size = 256 * 1024;
    let mut buf = vec![0u8; chunk_size];
    let mut last_progress = Instant::now();
    let progress_interval = std::time::Duration::from_millis(100);

    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RemoteError::Transfer("Transfer cancelled".into()));
        }

        let n = std::io::Read::read(&mut local_file, &mut buf)?;

        if n == 0 {
            break;
        }

        remote_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| RemoteError::Transfer(format!("write remote file: {e}")))?;

        bytes_transferred += n as u64;

        if last_progress.elapsed() >= progress_interval {
            last_progress = Instant::now();
            let _ = progress_tx.send(TransferProgress {
                transfer_id: transfer_id.to_string(),
                kind: TransferKind::Upload,
                status: TransferStatus::InProgress,
                bytes_transferred,
                total_bytes,
                file_name: file_name.to_string(),
                error: None,
            });
        }
    }

    finalize_remote_write(&mut remote_file, "finish remote upload").await?;

    Ok(bytes_transferred)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    struct FinalizationWriter {
        operations: Arc<Mutex<Vec<&'static str>>>,
    }

    impl tokio::io::AsyncWrite for FinalizationWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.operations.lock().push("flush");
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.operations.lock().push("shutdown");
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn finalizing_remote_file_flushes_then_closes_the_handle() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FinalizationWriter {
            operations: Arc::clone(&operations),
        };

        finalize_remote_write(&mut writer, "finish test transfer")
            .await
            .unwrap();

        assert_eq!(*operations.lock(), vec!["flush", "shutdown"]);
    }

    #[tokio::test]
    async fn closing_remote_read_handle_only_shuts_it_down() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FinalizationWriter {
            operations: Arc::clone(&operations),
        };

        close_remote_file(&mut writer, "finish test download")
            .await
            .unwrap();

        assert_eq!(*operations.lock(), vec!["shutdown"]);
    }

    #[test]
    fn transfer_progress_serializes() {
        let p = TransferProgress {
            transfer_id: "abc".into(),
            kind: TransferKind::Download,
            status: TransferStatus::InProgress,
            bytes_transferred: 1024,
            total_bytes: 4096,
            file_name: "test.txt".into(),
            error: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"download\""));
        assert!(json.contains("\"in_progress\""));
        assert!(json.contains("1024"));
    }

    #[test]
    fn transfer_registry_new_is_empty() {
        let reg = TransferRegistry::new();
        assert!(reg.transfers.is_empty());
    }

    #[test]
    fn transfer_registry_cancel_nonexistent() {
        let mut reg = TransferRegistry::new();
        assert!(!reg.cancel("nonexistent"));
    }
}
