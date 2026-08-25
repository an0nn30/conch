//! File transfer engine — upload/download with progress events.
//!
//! Uses SFTP for transfers. Future: rsync detection and fallback.

use serde::Serialize;
use ts_rs::TS;

use crate::error::RemoteError;

mod fingerprint;
pub use fingerprint::SourceFingerprint;

pub mod copy;
pub mod frontier;
pub mod sftp_io;
pub use copy::{ControlDecision, CopyOutcome, copy_with_checkpoint};
pub use sftp_io::{
    SftpFileHandle, SftpSessionHandle, download_to_partial, fingerprint_local_parts,
    fingerprint_open_local, fingerprint_open_remote, fingerprint_remote_parts, open_local_partial,
    open_remote_partial, open_sftp_session, truncate_local_partial, truncate_remote_partial,
    upload_to_partial,
};

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

    let flush_result = file
        .flush()
        .await
        .map_err(|e| RemoteError::Transfer(format!("{operation}: flush failed: {e}")));
    let shutdown_result = file
        .shutdown()
        .await
        .map_err(|e| RemoteError::Transfer(format!("{operation}: close failed: {e}")));
    flush_result.and(shutdown_result)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use parking_lot::Mutex;

    struct FinalizationWriter {
        operations: Arc<Mutex<Vec<&'static str>>>,
        fail_flush: bool,
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
            if self.fail_flush {
                Poll::Ready(Err(std::io::Error::other("flush broke")))
            } else {
                Poll::Ready(Ok(()))
            }
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
            fail_flush: false,
        };

        finalize_remote_write(&mut writer, "finish test transfer")
            .await
            .unwrap();

        assert_eq!(*operations.lock(), vec!["flush", "shutdown"]);
    }

    #[tokio::test]
    async fn finalizing_remote_file_still_closes_after_a_flush_error() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FinalizationWriter {
            operations: Arc::clone(&operations),
            fail_flush: true,
        };

        let error = finalize_remote_write(&mut writer, "finish test transfer")
            .await
            .unwrap_err();

        assert!(matches!(error, RemoteError::Transfer(message) if message.contains("flush broke")));
        assert_eq!(*operations.lock(), vec!["flush", "shutdown"]);
    }

    #[tokio::test]
    async fn closing_remote_read_handle_only_shuts_it_down() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FinalizationWriter {
            operations: Arc::clone(&operations),
            fail_flush: false,
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
}
