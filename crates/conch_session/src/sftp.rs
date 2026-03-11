use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// I/O buffer size for SFTP transfers (2 MB).
/// Larger buffers reduce per-packet SFTP protocol overhead.
const TRANSFER_BUF_SIZE: usize = 2 * 1024 * 1024;

/// A file entry returned by listing a directory.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Progress update for file transfers.
#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub bytes_transferred: u64,
    pub total_bytes: u64,
}

fn sort_entries(entries: &mut Vec<FileEntry>) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Local filesystem file provider.
pub struct LocalFileProvider;

impl LocalFileProvider {
    pub async fn list(&self, path: &Path) -> Result<Vec<FileEntry>> {
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(path)
            .await
            .with_context(|| format!("Failed to list {}", path.display()))?;

        while let Some(entry) = read_dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            entries.push(FileEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified,
            });
        }

        sort_entries(&mut entries);
        Ok(entries)
    }

    pub async fn download(
        &self,
        src: &Path,
        dst: &Path,
        _progress: Option<mpsc::UnboundedSender<TransferProgress>>,
    ) -> Result<()> {
        tokio::fs::copy(src, dst)
            .await
            .with_context(|| format!("Failed to copy {} to {}", src.display(), dst.display()))?;
        Ok(())
    }

    pub async fn upload(
        &self,
        local_path: &Path,
        remote_path: &Path,
        _progress: Option<mpsc::UnboundedSender<TransferProgress>>,
    ) -> Result<()> {
        tokio::fs::copy(local_path, remote_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    local_path.display(),
                    remote_path.display()
                )
            })?;
        Ok(())
    }

    pub async fn mkdir(&self, path: &Path) -> Result<()> {
        tokio::fs::create_dir_all(path)
            .await
            .with_context(|| format!("Failed to create directory {}", path.display()))?;
        Ok(())
    }

    pub async fn remove(&self, path: &Path) -> Result<()> {
        let meta = tokio::fs::metadata(path).await?;
        if meta.is_dir() {
            tokio::fs::remove_dir_all(path).await?;
        } else {
            tokio::fs::remove_file(path).await?;
        }
        Ok(())
    }
}

/// SFTP file provider backed by russh-sftp.
pub struct SftpFileProvider {
    sftp: russh_sftp::client::SftpSession,
}

impl SftpFileProvider {
    pub fn new(sftp: russh_sftp::client::SftpSession) -> Self {
        Self { sftp }
    }

    pub async fn list(&self, path: &Path) -> Result<Vec<FileEntry>> {
        let path_str = path.to_string_lossy().into_owned();
        let dir_entries = self
            .sftp
            .read_dir(path_str)
            .await
            .with_context(|| format!("SFTP: failed to list {}", path.display()))?;

        let mut entries = Vec::new();
        for entry in dir_entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let is_dir = entry.file_type().is_dir();
            let meta = entry.metadata();
            let size = meta.len();
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            entries.push(FileEntry {
                path: path.join(&name),
                name,
                is_dir,
                size,
                modified,
            });
        }

        sort_entries(&mut entries);
        Ok(entries)
    }

    /// Download a remote file to a local path with chunked progress reporting.
    pub async fn download(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress: Option<mpsc::UnboundedSender<TransferProgress>>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let remote_str = remote_path.to_string_lossy().into_owned();

        // Open remote file and get its size.
        let mut remote_file = self
            .sftp
            .open(&remote_str)
            .await
            .with_context(|| format!("SFTP: failed to open {}", remote_path.display()))?;
        let meta = remote_file
            .metadata()
            .await
            .with_context(|| format!("SFTP: failed to stat {}", remote_path.display()))?;
        let total = meta.len();

        // Create local file.
        let mut local_file = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("Failed to create {}", local_path.display()))?;

        let mut transferred: u64 = 0;
        let mut buf = vec![0u8; TRANSFER_BUF_SIZE];
        loop {
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let n = remote_file.read(&mut buf).await
                .with_context(|| format!("SFTP: read error on {}", remote_path.display()))?;
            if n == 0 {
                break;
            }
            local_file.write_all(&buf[..n]).await
                .with_context(|| format!("Failed to write {}", local_path.display()))?;
            transferred += n as u64;
            if let Some(tx) = &progress {
                let _ = tx.send(TransferProgress {
                    bytes_transferred: transferred,
                    total_bytes: total,
                });
            }
        }
        local_file.flush().await?;

        // Ensure final 100% progress is sent.
        if let Some(tx) = &progress {
            let _ = tx.send(TransferProgress {
                bytes_transferred: total,
                total_bytes: total,
            });
        }

        Ok(())
    }

    /// Upload a local file to a remote path with chunked progress reporting.
    pub async fn upload(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress: Option<mpsc::UnboundedSender<TransferProgress>>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        // Open local file and get its size.
        let mut local_file = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("Failed to open {}", local_path.display()))?;
        let local_meta = local_file.metadata().await
            .with_context(|| format!("Failed to stat {}", local_path.display()))?;
        let total = local_meta.len();

        // Create remote file.
        let remote_str = remote_path.to_string_lossy().into_owned();
        let mut remote_file = self
            .sftp
            .create(&remote_str)
            .await
            .with_context(|| format!("SFTP: failed to create {}", remote_path.display()))?;

        let mut transferred: u64 = 0;
        let mut buf = vec![0u8; TRANSFER_BUF_SIZE];
        loop {
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let n = local_file.read(&mut buf).await
                .with_context(|| format!("Failed to read {}", local_path.display()))?;
            if n == 0 {
                break;
            }
            remote_file.write_all(&buf[..n]).await
                .with_context(|| format!("SFTP: write error on {}", remote_path.display()))?;
            transferred += n as u64;
            if let Some(tx) = &progress {
                let _ = tx.send(TransferProgress {
                    bytes_transferred: transferred,
                    total_bytes: total,
                });
            }
        }
        remote_file.shutdown().await
            .with_context(|| format!("SFTP: failed to close {}", remote_path.display()))?;

        // Ensure final 100% progress is sent.
        if let Some(tx) = &progress {
            let _ = tx.send(TransferProgress {
                bytes_transferred: total,
                total_bytes: total,
            });
        }

        Ok(())
    }

    pub async fn mkdir(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy().into_owned();
        self.sftp
            .create_dir(path_str)
            .await
            .with_context(|| format!("SFTP: failed to mkdir {}", path.display()))?;
        Ok(())
    }

    pub async fn remove(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy().into_owned();
        // Try file first, then directory
        if self.sftp.remove_file(path_str.clone()).await.is_err() {
            self.sftp
                .remove_dir(path_str)
                .await
                .with_context(|| format!("SFTP: failed to remove {}", path.display()))?;
        }
        Ok(())
    }

    /// Resolve the remote home directory (canonicalize ".").
    pub async fn home_path(&self) -> Result<PathBuf> {
        let path = self
            .sftp
            .canonicalize(".")
            .await
            .map_err(|e| anyhow::anyhow!("SFTP canonicalize: {e}"))?;
        Ok(PathBuf::from(path))
    }
}

// ---------------------------------------------------------------------------
// SFTP background worker
// ---------------------------------------------------------------------------

/// Commands sent from the UI thread to the SFTP worker.
pub enum SftpCmd {
    /// List the given remote directory.
    List(PathBuf),
    /// Upload a local file to a remote directory.
    Upload {
        local_path: PathBuf,
        remote_dir: PathBuf,
        cancel: Arc<AtomicBool>,
    },
    /// Download a remote file to a local directory.
    Download {
        remote_path: PathBuf,
        local_dir: PathBuf,
        cancel: Arc<AtomicBool>,
    },
    /// Shut down the SFTP worker.
    Shutdown,
}

/// A directory listing result sent back from the SFTP worker.
pub struct SftpListing {
    pub path: PathBuf,
    pub entries: Vec<FileEntry>,
    pub home: PathBuf,
}

/// Events sent from the SFTP worker back to the UI.
pub enum SftpEvent {
    /// A directory listing completed.
    Listing(SftpListing),
    /// Incremental transfer progress update.
    TransferProgress {
        filename: String,
        bytes_transferred: u64,
        total_bytes: u64,
    },
    /// A file transfer completed (upload or download).
    TransferComplete {
        filename: String,
        success: bool,
        error: Option<String>,
    },
    /// Indicates whether rsync is being used for transfers.
    RsyncAvailable(bool),
}

/// Long-running async task that owns an `SftpFileProvider` and serves listing
/// and transfer requests over channels. When `connect_info` is provided and
/// rsync is available on both sides, file transfers use rsync over SSH instead
/// of SFTP for better performance.
pub async fn run_sftp_worker(
    ssh_handle: Arc<russh::client::Handle<crate::ssh::client::ClientHandler>>,
    mut cmd_rx: mpsc::UnboundedReceiver<SftpCmd>,
    result_tx: std::sync::mpsc::Sender<SftpEvent>,
    connect_info: Option<crate::ssh::session::SshConnectInfo>,
) {
    // Open an SFTP channel.
    let channel = match ssh_handle.channel_open_session().await {
        Ok(ch) => ch,
        Err(e) => {
            log::error!("SFTP: failed to open channel: {e}");
            return;
        }
    };
    if let Err(e) = channel.request_subsystem(true, "sftp").await {
        log::error!("SFTP: failed to request subsystem: {e}");
        return;
    }
    let sftp_session = match russh_sftp::client::SftpSession::new(channel.into_stream()).await {
        Ok(s) => s,
        Err(e) => {
            log::error!("SFTP: failed to init session: {e}");
            return;
        }
    };
    let provider = SftpFileProvider::new(sftp_session);

    // Resolve the remote home directory.
    let home = match provider.home_path().await {
        Ok(h) => h,
        Err(e) => {
            log::error!("SFTP: failed to resolve home: {e}");
            return;
        }
    };

    // List home directory immediately.
    if let Ok(entries) = provider.list(&home).await {
        let _ = result_tx.send(SftpEvent::Listing(SftpListing {
            path: home.clone(),
            entries,
            home: home.clone(),
        }));
    }

    // Check rsync availability and compression support once at worker startup.
    let (use_rsync, use_zstd) = if connect_info.is_some() {
        let (local_check, remote_check) = tokio::join!(
            crate::rsync::check_local_rsync(),
            crate::rsync::check_remote_rsync(&ssh_handle),
        );
        if local_check.available && remote_check.available {
            let zstd = local_check.has_zstd && remote_check.has_zstd;
            let compress = if zstd { "zstd" } else { "zlib" };
            log::info!("rsync available on both sides (compression: {compress})");
            (true, zstd)
        } else {
            log::info!(
                "rsync not available (local={}, remote={}) — using SFTP",
                local_check.available,
                remote_check.available,
            );
            (false, false)
        }
    } else {
        (false, false)
    };

    // Notify the UI about the transfer method.
    let _ = result_tx.send(SftpEvent::RsyncAvailable(use_rsync));

    // Command loop.
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            SftpCmd::List(path) => {
                // Resolve relative paths (e.g. ".") to absolute via SFTP canonicalize.
                let resolved = if path.is_relative() {
                    let p = path.to_string_lossy();
                    match provider.sftp.canonicalize(p.as_ref()).await {
                        Ok(abs) => PathBuf::from(abs),
                        Err(_) => path.clone(),
                    }
                } else {
                    path.clone()
                };
                match provider.list(&resolved).await {
                    Ok(entries) => {
                        let _ = result_tx.send(SftpEvent::Listing(SftpListing {
                            path: resolved,
                            entries,
                            home: home.clone(),
                        }));
                    }
                    Err(e) => {
                        log::error!("SFTP: list failed for {}: {e}", resolved.display());
                    }
                }
            }
            SftpCmd::Upload { local_path, remote_dir, cancel } => {
                let filename = local_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let remote_path = remote_dir.join(&filename);

                // Progress channel: forward incremental updates to the UI.
                let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<TransferProgress>();
                let prog_result_tx = result_tx.clone();
                let prog_filename = filename.clone();
                let prog_handle = tokio::spawn(async move {
                    while let Some(p) = prog_rx.recv().await {
                        let _ = prog_result_tx.send(SftpEvent::TransferProgress {
                            filename: prog_filename.clone(),
                            bytes_transferred: p.bytes_transferred,
                            total_bytes: p.total_bytes,
                        });
                    }
                });

                let result = if use_rsync {
                    let info = connect_info.as_ref().unwrap();
                    match crate::rsync::rsync_upload(
                        info, &local_path, &remote_path, &cancel, Some(prog_tx), use_zstd,
                    ).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("rsync upload failed, falling back to SFTP: {e}");
                            let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel::<TransferProgress>();
                            let fb_result_tx = result_tx.clone();
                            let fb_filename = filename.clone();
                            let fb_handle = tokio::spawn(async move {
                                while let Some(p) = fallback_rx.recv().await {
                                    let _ = fb_result_tx.send(SftpEvent::TransferProgress {
                                        filename: fb_filename.clone(),
                                        bytes_transferred: p.bytes_transferred,
                                        total_bytes: p.total_bytes,
                                    });
                                }
                            });
                            let r = provider.upload(&local_path, &remote_path, Some(fallback_tx), &cancel).await;
                            let _ = fb_handle.await;
                            r
                        }
                    }
                } else {
                    provider.upload(&local_path, &remote_path, Some(prog_tx), &cancel).await
                };

                match result {
                    Ok(()) => {
                        let _ = result_tx.send(SftpEvent::TransferComplete {
                            filename,
                            success: true,
                            error: None,
                        });
                    }
                    Err(e) => {
                        let _ = result_tx.send(SftpEvent::TransferComplete {
                            filename,
                            success: false,
                            error: Some(e.to_string()),
                        });
                    }
                }
                let _ = prog_handle.await;
            }
            SftpCmd::Download { remote_path, local_dir, cancel } => {
                let filename = remote_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let local_path = local_dir.join(&filename);

                // Progress channel: forward incremental updates to the UI.
                let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<TransferProgress>();
                let prog_result_tx = result_tx.clone();
                let prog_filename = filename.clone();
                let prog_handle = tokio::spawn(async move {
                    while let Some(p) = prog_rx.recv().await {
                        let _ = prog_result_tx.send(SftpEvent::TransferProgress {
                            filename: prog_filename.clone(),
                            bytes_transferred: p.bytes_transferred,
                            total_bytes: p.total_bytes,
                        });
                    }
                });

                let result = if use_rsync {
                    let info = connect_info.as_ref().unwrap();
                    match crate::rsync::rsync_download(
                        info, &remote_path, &local_path, &cancel, Some(prog_tx), use_zstd,
                    ).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("rsync download failed, falling back to SFTP: {e}");
                            let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel::<TransferProgress>();
                            let fb_result_tx = result_tx.clone();
                            let fb_filename = filename.clone();
                            let fb_handle = tokio::spawn(async move {
                                while let Some(p) = fallback_rx.recv().await {
                                    let _ = fb_result_tx.send(SftpEvent::TransferProgress {
                                        filename: fb_filename.clone(),
                                        bytes_transferred: p.bytes_transferred,
                                        total_bytes: p.total_bytes,
                                    });
                                }
                            });
                            let r = provider.download(&remote_path, &local_path, Some(fallback_tx), &cancel).await;
                            let _ = fb_handle.await;
                            r
                        }
                    }
                } else {
                    provider.download(&remote_path, &local_path, Some(prog_tx), &cancel).await
                };

                match result {
                    Ok(()) => {
                        let _ = result_tx.send(SftpEvent::TransferComplete {
                            filename,
                            success: true,
                            error: None,
                        });
                    }
                    Err(e) => {
                        let _ = result_tx.send(SftpEvent::TransferComplete {
                            filename,
                            success: false,
                            error: Some(e.to_string()),
                        });
                    }
                }
                let _ = prog_handle.await;
            }
            SftpCmd::Shutdown => break,
        }
    }

    log::info!("SFTP worker shut down");
}
