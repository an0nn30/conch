//! File transfer engine — upload/download with progress events.
//!
//! Uses SFTP for transfers. Future: rsync detection and fallback.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::handler::ConchSshHandler;
use crate::sftp;
use crate::sftp::open_sftp;

// ---------------------------------------------------------------------------
// Transfer types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    Download,
    Upload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Serialize)]
pub struct TransferProgress {
    pub transfer_id: String,
    pub kind: TransferKind,
    pub status: TransferStatus,
    pub bytes_transferred: u64,
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
    ssh_handle: Arc<russh::client::Handle<ConchSshHandler>>,
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
        .await;

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
    ssh: &russh::client::Handle<ConchSshHandler>,
    remote_path: &str,
    local_path: &str,
    cancelled: &AtomicBool,
    transfer_id: &str,
    file_name: &str,
    progress_tx: &mpsc::UnboundedSender<TransferProgress>,
) -> Result<u64, String> {
    use std::time::Instant;
    use tokio::io::AsyncReadExt;

    let stat = sftp::stat(ssh, remote_path).await?;
    let total_bytes = stat.size;

    let sftp_session = open_sftp(ssh).await?;
    let mut remote_file = sftp_session
        .open(remote_path)
        .await
        .map_err(|e| format!("open failed: {e}"))?;

    let mut local_file =
        std::fs::File::create(local_path).map_err(|e| format!("create local file: {e}"))?;

    let mut bytes_transferred: u64 = 0;
    let chunk_size = 256 * 1024;
    let mut buf = vec![0u8; chunk_size];
    let mut last_progress = Instant::now();
    let progress_interval = std::time::Duration::from_millis(100);

    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Transfer cancelled".to_string());
        }

        let n = remote_file
            .read(&mut buf)
            .await
            .map_err(|e| format!("read failed: {e}"))?;

        if n == 0 {
            break;
        }

        std::io::Write::write_all(&mut local_file, &buf[..n])
            .map_err(|e| format!("write local file: {e}"))?;

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

    Ok(bytes_transferred)
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// Start a background upload from local to remote.
/// Returns the transfer_id.
pub fn start_upload(
    transfer_id: String,
    ssh_handle: Arc<russh::client::Handle<ConchSshHandler>>,
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
        .await;

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
    ssh: &russh::client::Handle<ConchSshHandler>,
    local_path: &str,
    remote_path: &str,
    cancelled: &AtomicBool,
    transfer_id: &str,
    file_name: &str,
    progress_tx: &mpsc::UnboundedSender<TransferProgress>,
) -> Result<u64, String> {
    use std::time::Instant;
    use tokio::io::AsyncWriteExt;

    let local_meta = std::fs::metadata(local_path).map_err(|e| format!("stat local file: {e}"))?;
    let total_bytes = local_meta.len();

    let mut local_file =
        std::fs::File::open(local_path).map_err(|e| format!("open local file: {e}"))?;

    let sftp_session = open_sftp(ssh).await?;
    let mut remote_file = sftp_session
        .create(remote_path)
        .await
        .map_err(|e| format!("create remote file: {e}"))?;

    let mut bytes_transferred: u64 = 0;
    let chunk_size = 256 * 1024;
    let mut buf = vec![0u8; chunk_size];
    let mut last_progress = Instant::now();
    let progress_interval = std::time::Duration::from_millis(100);

    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Transfer cancelled".to_string());
        }

        let n = std::io::Read::read(&mut local_file, &mut buf)
            .map_err(|e| format!("read local file: {e}"))?;

        if n == 0 {
            break;
        }

        remote_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| format!("write remote file: {e}"))?;

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

    Ok(bytes_transferred)
}

#[cfg(test)]
mod tests {
    use super::*;

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
