use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

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

    pub async fn download(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress: Option<mpsc::UnboundedSender<TransferProgress>>,
    ) -> Result<()> {
        let remote_str = remote_path.to_string_lossy().into_owned();
        let data = self
            .sftp
            .read(remote_str)
            .await
            .with_context(|| format!("SFTP: failed to read {}", remote_path.display()))?;

        let total = data.len() as u64;
        tokio::fs::write(local_path, &data)
            .await
            .with_context(|| format!("Failed to write {}", local_path.display()))?;

        if let Some(tx) = progress {
            let _ = tx.send(TransferProgress {
                bytes_transferred: total,
                total_bytes: total,
            });
        }

        Ok(())
    }

    pub async fn upload(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress: Option<mpsc::UnboundedSender<TransferProgress>>,
    ) -> Result<()> {
        let data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("Failed to read {}", local_path.display()))?;

        let total = data.len() as u64;
        let remote_str = remote_path.to_string_lossy().into_owned();
        self.sftp
            .write(remote_str, &data)
            .await
            .with_context(|| format!("SFTP: failed to write {}", remote_path.display()))?;

        if let Some(tx) = progress {
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
    /// Shut down the SFTP worker.
    Shutdown,
}

/// A directory listing result sent back from the SFTP worker.
pub struct SftpListing {
    pub path: PathBuf,
    pub entries: Vec<FileEntry>,
    pub home: PathBuf,
}

/// Long-running async task that owns an `SftpFileProvider` and serves listing
/// requests over channels.
pub async fn run_sftp_worker(
    ssh_handle: Arc<russh::client::Handle<crate::ssh::client::ClientHandler>>,
    mut cmd_rx: mpsc::UnboundedReceiver<SftpCmd>,
    result_tx: std::sync::mpsc::Sender<SftpListing>,
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
        let _ = result_tx.send(SftpListing {
            path: home.clone(),
            entries,
            home: home.clone(),
        });
    }

    // Command loop.
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            SftpCmd::List(path) => {
                match provider.list(&path).await {
                    Ok(entries) => {
                        let _ = result_tx.send(SftpListing {
                            path,
                            entries,
                            home: home.clone(),
                        });
                    }
                    Err(e) => {
                        log::error!("SFTP: list failed for {}: {e}", path.display());
                    }
                }
            }
            SftpCmd::Shutdown => break,
        }
    }

    log::info!("SFTP worker shut down");
}
