//! SFTP operations — list, stat, read, write, mkdir, rename, delete.
//!
//! Each operation opens an SFTP subsystem channel on demand via the stored
//! SSH handle. No vtables, no ref counting — just direct async calls.

use russh_sftp::client::SftpSession;
use serde::Serialize;

use crate::handler::ConchSshHandler;

/// A file entry returned from SFTP or local filesystem operations.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
    pub permissions: Option<String>,
}

/// Open an SFTP session on the given SSH handle.
pub(crate) async fn open_sftp(
    ssh: &russh::client::Handle<ConchSshHandler>,
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
    ssh: &russh::client::Handle<ConchSshHandler>,
    path: &str,
) -> Result<Vec<FileEntry>, String> {
    let sftp = open_sftp(ssh).await?;
    let entries = sftp
        .read_dir(path)
        .await
        .map_err(|e| format!("read_dir failed: {e}"))?;

    Ok(entries
        .map(|entry| {
            let meta = entry.metadata();
            FileEntry {
                name: entry.file_name(),
                is_dir: meta.is_dir(),
                size: meta.size.unwrap_or(0),
                modified: meta.mtime.map(|t| t as u64),
                permissions: meta.permissions.map(|p| format!("{:o}", p)),
            }
        })
        .collect())
}

/// Stat a single path.
pub async fn stat(
    ssh: &russh::client::Handle<ConchSshHandler>,
    path: &str,
) -> Result<FileEntry, String> {
    let sftp = open_sftp(ssh).await?;
    let attrs = sftp
        .metadata(path)
        .await
        .map_err(|e| format!("stat failed: {e}"))?;

    // Extract filename from path.
    let name = path.rsplit('/').next().unwrap_or(path).to_string();

    Ok(FileEntry {
        name,
        is_dir: attrs.is_dir(),
        size: attrs.size.unwrap_or(0),
        modified: attrs.mtime.map(|t| t as u64),
        permissions: attrs.permissions.map(|p| format!("{:o}", p)),
    })
}

/// Read file contents (up to `length` bytes from `offset`), returned as base64.
pub async fn read_file(
    ssh: &russh::client::Handle<ConchSshHandler>,
    path: &str,
    offset: u64,
    length: usize,
) -> Result<ReadFileResult, String> {
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

    let cap = length.min(1024 * 1024); // cap at 1MB
    let mut buf = vec![0u8; cap];
    let n = file
        .read(&mut buf)
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    buf.truncate(n);

    let data = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(ReadFileResult {
        data,
        bytes_read: n as u64,
    })
}

#[derive(Serialize)]
pub struct ReadFileResult {
    pub data: String,
    pub bytes_read: u64,
}

/// Write data to a file (base64-encoded input).
pub async fn write_file(
    ssh: &russh::client::Handle<ConchSshHandler>,
    path: &str,
    data_b64: &str,
) -> Result<u64, String> {
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

    Ok(data.len() as u64)
}

/// Create a directory.
pub async fn mkdir(
    ssh: &russh::client::Handle<ConchSshHandler>,
    path: &str,
) -> Result<(), String> {
    let sftp = open_sftp(ssh).await?;
    sftp.create_dir(path)
        .await
        .map_err(|e| format!("mkdir failed: {e}"))
}

/// Rename a file or directory.
pub async fn rename(
    ssh: &russh::client::Handle<ConchSshHandler>,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let sftp = open_sftp(ssh).await?;
    sftp.rename(from, to)
        .await
        .map_err(|e| format!("rename failed: {e}"))
}

/// Delete a file or directory.
pub async fn remove(
    ssh: &russh::client::Handle<ConchSshHandler>,
    path: &str,
    is_dir: bool,
) -> Result<(), String> {
    let sftp = open_sftp(ssh).await?;
    if is_dir {
        sftp.remove_dir(path)
            .await
            .map_err(|e| format!("rmdir failed: {e}"))
    } else {
        sftp.remove_file(path)
            .await
            .map_err(|e| format!("remove failed: {e}"))
    }
}

/// Resolve a path to its canonical absolute form.
pub async fn realpath(
    ssh: &russh::client::Handle<ConchSshHandler>,
    path: &str,
) -> Result<String, String> {
    let sftp = open_sftp(ssh).await?;
    sftp.canonicalize(path)
        .await
        .map_err(|e| format!("realpath failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_entry_serializes() {
        let entry = FileEntry {
            name: "test.txt".to_string(),
            is_dir: false,
            size: 1024,
            modified: Some(1700000000),
            permissions: Some("644".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("test.txt"));
        assert!(json.contains("1024"));
    }

    #[test]
    fn file_entry_dir_serializes() {
        let entry = FileEntry {
            name: "subdir".to_string(),
            is_dir: true,
            size: 0,
            modified: None,
            permissions: Some("755".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"is_dir\":true"));
    }
}
