//! Rsync-based file transfer over SSH.
//!
//! Tries to use rsync for uploads/downloads when available on both the local
//! machine and the remote server. Falls back to SFTP transparently.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::sftp::TransferProgress;
use crate::ssh::session::SshConnectInfo;

/// Result of checking rsync on one side (local or remote).
#[derive(Debug, Clone, Copy)]
pub struct RsyncCheck {
    /// rsync binary is available.
    pub available: bool,
    /// rsync supports zstd compression (3.2.3+).
    pub has_zstd: bool,
}

/// Check rsync availability and zstd support on the local machine.
pub async fn check_local_rsync() -> RsyncCheck {
    let output = tokio::process::Command::new("rsync")
        .arg("--version")
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => {
            let version_str = String::from_utf8_lossy(&out.stdout);
            RsyncCheck {
                available: true,
                has_zstd: version_str.contains("zstd"),
            }
        }
        _ => RsyncCheck {
            available: false,
            has_zstd: false,
        },
    }
}

/// Check rsync availability and zstd support on the remote via SSH.
pub async fn check_remote_rsync(
    ssh_handle: &Arc<russh::client::Handle<crate::ssh::client::ClientHandler>>,
) -> RsyncCheck {
    match crate::ssh::session::ssh_exec_command(
        Arc::clone(ssh_handle),
        "rsync --version 2>/dev/null".to_string(),
    )
    .await
    {
        Ok(output) if !output.trim().is_empty() => RsyncCheck {
            available: true,
            has_zstd: output.contains("zstd"),
        },
        _ => RsyncCheck {
            available: false,
            has_zstd: false,
        },
    }
}

/// Build the SSH command string for rsync's `-e` flag.
fn build_ssh_cmd(info: &SshConnectInfo) -> String {
    let mut parts = vec!["ssh".to_string()];
    if info.port != 22 {
        parts.push(format!("-p {}", info.port));
    }
    if let Some(ref key) = info.identity_file {
        parts.push(format!("-i {}", key.display()));
    }
    // Disable strict host key checking since we already verified via russh.
    parts.push("-o StrictHostKeyChecking=no".to_string());
    parts.push("-o BatchMode=yes".to_string());
    parts.join(" ")
}

/// Build the compression arguments for rsync.
/// Uses zstd at max level when available, otherwise zlib at max level.
fn compression_args(use_zstd: bool) -> Vec<&'static str> {
    if use_zstd {
        vec!["--zc=zstd", "--compress-level=19"]
    } else {
        vec!["-z", "--compress-level=9"]
    }
}

/// Upload a local file to a remote path using rsync over SSH.
pub async fn rsync_upload(
    info: &SshConnectInfo,
    local_path: &Path,
    remote_path: &Path,
    cancel: &AtomicBool,
    progress: Option<mpsc::UnboundedSender<TransferProgress>>,
    use_zstd: bool,
) -> Result<()> {
    let total = tokio::fs::metadata(local_path)
        .await
        .with_context(|| format!("Failed to stat {}", local_path.display()))?
        .len();

    let ssh_cmd = build_ssh_cmd(info);
    let remote_dest = format!(
        "{}@{}:{}",
        info.user,
        info.host,
        remote_path.display()
    );

    let mut args = vec![
        "-a".to_string(),
        "--info=progress2".to_string(),
        "--no-human-readable".to_string(),
        "-e".to_string(),
        ssh_cmd,
    ];
    for arg in compression_args(use_zstd) {
        args.push(arg.to_string());
    }
    args.push(local_path.to_string_lossy().into_owned());
    args.push(remote_dest);

    let mut child = tokio::process::Command::new("rsync")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn rsync")?;

    // Parse rsync --info=progress2 output for progress updates.
    if let Some(stdout) = child.stdout.take() {
        let progress = progress.clone();
        let cancel_clone = Arc::new(AtomicBool::new(false));
        let cancel_ref = cancel_clone.clone();
        tokio::spawn(async move {
            parse_rsync_progress(stdout, total, progress, &cancel_ref).await;
        });

        // Poll cancellation.
        loop {
            if cancel.load(Ordering::Relaxed) {
                cancel_clone.store(true, Ordering::Relaxed);
                let _ = child.kill().await;
                anyhow::bail!("cancelled");
            }
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                child.wait(),
            )
            .await
            {
                Ok(Ok(status)) => {
                    if !status.success() {
                        let stderr = if let Some(mut err) = child.stderr.take() {
                            let mut buf = String::new();
                            use tokio::io::AsyncReadExt;
                            let _ = err.read_to_string(&mut buf).await;
                            buf
                        } else {
                            String::new()
                        };
                        anyhow::bail!("rsync failed: {}", stderr.trim());
                    }
                    break;
                }
                Ok(Err(e)) => anyhow::bail!("rsync process error: {e}"),
                Err(_) => continue, // Timeout — keep polling.
            }
        }
    } else {
        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("rsync failed with exit code {:?}", status.code());
        }
    }

    // Send final 100% progress.
    if let Some(tx) = &progress {
        let _ = tx.send(TransferProgress {
            bytes_transferred: total,
            total_bytes: total,
        });
    }

    Ok(())
}

/// Download a remote file to a local path using rsync over SSH.
pub async fn rsync_download(
    info: &SshConnectInfo,
    remote_path: &Path,
    local_path: &Path,
    cancel: &AtomicBool,
    progress: Option<mpsc::UnboundedSender<TransferProgress>>,
    use_zstd: bool,
) -> Result<()> {
    let ssh_cmd = build_ssh_cmd(info);
    let remote_src = format!(
        "{}@{}:{}",
        info.user,
        info.host,
        remote_path.display()
    );

    let mut args = vec![
        "-a".to_string(),
        "--info=progress2".to_string(),
        "--no-human-readable".to_string(),
        "-e".to_string(),
        ssh_cmd,
    ];
    for arg in compression_args(use_zstd) {
        args.push(arg.to_string());
    }
    args.push(remote_src);
    args.push(local_path.to_string_lossy().into_owned());

    // We don't know the total size upfront for downloads, so we'll report
    // percentage-based progress from rsync's output.
    let mut child = tokio::process::Command::new("rsync")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn rsync")?;

    if let Some(stdout) = child.stdout.take() {
        let progress = progress.clone();
        let cancel_clone = Arc::new(AtomicBool::new(false));
        let cancel_ref = cancel_clone.clone();
        tokio::spawn(async move {
            // Use 0 as total — parse_rsync_progress will use percentage.
            parse_rsync_progress(stdout, 0, progress, &cancel_ref).await;
        });

        loop {
            if cancel.load(Ordering::Relaxed) {
                cancel_clone.store(true, Ordering::Relaxed);
                let _ = child.kill().await;
                anyhow::bail!("cancelled");
            }
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                child.wait(),
            )
            .await
            {
                Ok(Ok(status)) => {
                    if !status.success() {
                        let stderr = if let Some(mut err) = child.stderr.take() {
                            let mut buf = String::new();
                            use tokio::io::AsyncReadExt;
                            let _ = err.read_to_string(&mut buf).await;
                            buf
                        } else {
                            String::new()
                        };
                        anyhow::bail!("rsync failed: {}", stderr.trim());
                    }
                    break;
                }
                Ok(Err(e)) => anyhow::bail!("rsync process error: {e}"),
                Err(_) => continue,
            }
        }
    } else {
        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("rsync failed with exit code {:?}", status.code());
        }
    }

    // Send final progress — for downloads, read the actual file size.
    if let Some(tx) = &progress {
        let size = tokio::fs::metadata(local_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let _ = tx.send(TransferProgress {
            bytes_transferred: size,
            total_bytes: size,
        });
    }

    Ok(())
}

/// Parse rsync --info=progress2 --no-human-readable stdout.
/// Lines look like: "    1,234,567  45%   12.34MB/s    0:00:03"
async fn parse_rsync_progress(
    stdout: tokio::process::ChildStdout,
    total: u64,
    progress: Option<mpsc::UnboundedSender<TransferProgress>>,
    cancel: &AtomicBool,
) {
    use tokio::io::AsyncReadExt;

    // rsync --info=progress2 uses \r (carriage return) to update the same
    // line in-place, so we can't use lines() which splits on \n.
    // Read raw bytes and split on \r instead.
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut buf = [0u8; 512];
    let mut accum = String::new();

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]);
                accum.push_str(&chunk);

                // Split on \r or \n — each segment is a progress update.
                while let Some(pos) = accum.find(['\r', '\n']) {
                    let segment = accum[..pos].to_string();
                    accum = accum[pos + 1..].to_string();

                    if let Some(pct) = progress.as_ref().and_then(|_| extract_percentage(&segment)) {
                        let tx = progress.as_ref().unwrap();
                        if total > 0 {
                            let transferred = (total as f64 * pct / 100.0) as u64;
                            let _ = tx.send(TransferProgress {
                                bytes_transferred: transferred,
                                total_bytes: total,
                            });
                        } else {
                            let _ = tx.send(TransferProgress {
                                bytes_transferred: pct as u64,
                                total_bytes: 100,
                            });
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
}

/// Extract percentage from rsync progress2 output.
/// Matches patterns like "45%" in the progress line.
fn extract_percentage(line: &str) -> Option<f64> {
    for part in line.split_whitespace() {
        if let Some(pct) = part
            .strip_suffix('%')
            .and_then(|s| s.parse::<f64>().ok())
        {
            return Some(pct);
        }
    }
    None
}
