//! SFTP operations — list_dir, stat, read_file, write_file, mkdir, rename, delete.
//!
//! Each operation opens the SFTP subsystem on demand via the stored SSH handle.
//! The SFTP session is created per-call to avoid lifetime issues with the
//! async runtime boundary.

use russh_sftp::client::SftpSession;
use serde_json::{json, Value};

/// Open an SFTP session on the given SSH handle.
async fn open_sftp(
    ssh: &russh::client::Handle<crate::SshHandler>,
) -> Result<SftpSession, String> {
    let channel = ssh
        .channel_open_session()
        .await
        .map_err(|e| format!("failed to open SFTP channel: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("SFTP subsystem request failed: {e}"))?;
    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("SFTP session init failed: {e}"))
}

/// List directory entries at `path`.
pub async fn list_dir(
    ssh: &russh::client::Handle<crate::SshHandler>,
    path: &str,
) -> Result<Value, String> {
    let sftp = open_sftp(ssh).await?;
    let entries = sftp
        .read_dir(path)
        .await
        .map_err(|e| format!("read_dir failed: {e}"))?;

    let items: Vec<Value> = entries
        .map(|entry| {
            let meta = entry.metadata();
            json!({
                "name": entry.file_name(),
                "size": meta.size.unwrap_or(0),
                "is_dir": meta.is_dir(),
                "permissions": meta.permissions.map(|p| format!("{:o}", p)),
                "mtime": meta.mtime.unwrap_or(0),
            })
        })
        .collect();

    Ok(json!({ "status": "ok", "entries": items }))
}

/// Stat a single path.
pub async fn stat(
    ssh: &russh::client::Handle<crate::SshHandler>,
    path: &str,
) -> Result<Value, String> {
    let sftp = open_sftp(ssh).await?;
    let attrs = sftp
        .metadata(path)
        .await
        .map_err(|e| format!("stat failed: {e}"))?;

    Ok(json!({
        "status": "ok",
        "size": attrs.size.unwrap_or(0),
        "is_dir": attrs.is_dir(),
        "permissions": attrs.permissions.map(|p| format!("{:o}", p)),
    }))
}

/// Read file contents (up to `length` bytes from `offset`).
pub async fn read_file(
    ssh: &russh::client::Handle<crate::SshHandler>,
    path: &str,
    offset: u64,
    length: usize,
) -> Result<Value, String> {
    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let sftp = open_sftp(ssh).await?;
    let mut file = sftp
        .open(path)
        .await
        .map_err(|e| format!("open failed: {e}"))?;

    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| format!("seek failed: {e}"))?;
    }

    let mut buf = vec![0u8; length.min(1024 * 1024)]; // cap at 1MB
    let n = file
        .read(&mut buf)
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    buf.truncate(n);

    let encoded = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(json!({
        "status": "ok",
        "data": encoded,
        "bytes_read": n,
    }))
}

/// Write data to a file (base64-encoded `data`).
pub async fn write_file(
    ssh: &russh::client::Handle<crate::SshHandler>,
    path: &str,
    data_b64: &str,
) -> Result<Value, String> {
    use base64::Engine;
    use tokio::io::AsyncWriteExt;

    let data = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| format!("invalid base64: {e}"))?;

    let sftp = open_sftp(ssh).await?;
    let mut file = sftp
        .create(path)
        .await
        .map_err(|e| format!("create failed: {e}"))?;

    file.write_all(&data)
        .await
        .map_err(|e| format!("write failed: {e}"))?;

    Ok(json!({ "status": "ok", "bytes_written": data.len() }))
}

/// Create a directory.
pub async fn mkdir(
    ssh: &russh::client::Handle<crate::SshHandler>,
    path: &str,
) -> Result<Value, String> {
    let sftp = open_sftp(ssh).await?;
    sftp.create_dir(path)
        .await
        .map_err(|e| format!("mkdir failed: {e}"))?;
    Ok(json!({ "status": "ok" }))
}

/// Rename a file or directory.
pub async fn rename(
    ssh: &russh::client::Handle<crate::SshHandler>,
    from: &str,
    to: &str,
) -> Result<Value, String> {
    let sftp = open_sftp(ssh).await?;
    sftp.rename(from, to)
        .await
        .map_err(|e| format!("rename failed: {e}"))?;
    Ok(json!({ "status": "ok" }))
}

/// Delete a file.
pub async fn remove_file(
    ssh: &russh::client::Handle<crate::SshHandler>,
    path: &str,
) -> Result<Value, String> {
    let sftp = open_sftp(ssh).await?;
    sftp.remove_file(path)
        .await
        .map_err(|e| format!("remove failed: {e}"))?;
    Ok(json!({ "status": "ok" }))
}

/// Remove a directory.
pub async fn remove_dir(
    ssh: &russh::client::Handle<crate::SshHandler>,
    path: &str,
) -> Result<Value, String> {
    let sftp = open_sftp(ssh).await?;
    sftp.remove_dir(path)
        .await
        .map_err(|e| format!("rmdir failed: {e}"))?;
    Ok(json!({ "status": "ok" }))
}
