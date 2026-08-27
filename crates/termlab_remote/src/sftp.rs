//! SFTP operations — list, stat, read, write, mkdir, rename, delete.
//!
//! Each operation opens an SFTP subsystem channel on demand via the stored
//! SSH handle. No vtables, no ref counting — just direct async calls.

use russh_sftp::client::SftpSession;
use serde::Serialize;
use ts_rs::TS;

use crate::error::RemoteError;
use crate::handler::TermLabSshHandler;

/// A file entry returned from SFTP or local filesystem operations.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    #[ts(as = "f64")]
    pub size: u64,
    #[ts(as = "Option<f64>")]
    pub modified: Option<u64>,
    pub permissions: Option<String>,
}

/// Open an SFTP session on the given SSH handle.
pub(crate) async fn open_sftp(
    ssh: &russh::client::Handle<TermLabSshHandler>,
) -> Result<SftpSession, RemoteError> {
    let channel = ssh
        .channel_open_session()
        .await
        .map_err(|e| RemoteError::Sftp(format!("failed to open SFTP channel: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| RemoteError::Sftp(format!("SFTP subsystem request failed: {e}")))?;
    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| RemoteError::Sftp(format!("SFTP session init failed: {e}")))
}

/// List directory entries at `path`.
pub async fn list_dir(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
) -> Result<Vec<FileEntry>, RemoteError> {
    let sftp = open_sftp(ssh).await?;
    let entries = sftp
        .read_dir(path)
        .await
        .map_err(|e| RemoteError::Sftp(format!("read_dir failed: {e}")))?;

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

/// What a directory child is, resolved WITHOUT following symlinks.
///
/// `FileEntry` deliberately collapses this to `is_dir` for the file browser.
/// A recursive walk cannot: it must descend directories, transfer files, and
/// refuse to follow links (which risk cycles and surprise trees).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirEntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

/// One directory child with its unresolved type and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirChild {
    pub name: String,
    pub kind: DirEntryKind,
    pub size: u64,
}

/// List directory children at `path`, typed for a recursive walk. `.` and
/// `..` are filtered out by the underlying reader.
pub async fn list_dir_children(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
) -> Result<Vec<DirChild>, RemoteError> {
    use russh_sftp::protocol::FileType;

    let sftp = open_sftp(ssh).await?;
    let entries = sftp
        .read_dir(path)
        .await
        .map_err(|e| RemoteError::Sftp(format!("read_dir failed: {e}")))?;

    Ok(entries
        .map(|entry| {
            let metadata = entry.metadata();
            DirChild {
                name: entry.file_name(),
                kind: match entry.file_type() {
                    FileType::Dir => DirEntryKind::Dir,
                    FileType::File => DirEntryKind::File,
                    FileType::Symlink => DirEntryKind::Symlink,
                    FileType::Other => DirEntryKind::Other,
                },
                size: metadata.size.unwrap_or(0),
            }
        })
        .collect())
}

/// Stat a single path.
pub async fn stat(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
) -> Result<FileEntry, RemoteError> {
    let sftp = open_sftp(ssh).await?;
    let attrs = sftp
        .metadata(path)
        .await
        .map_err(|e| RemoteError::Sftp(format!("stat failed: {e}")))?;

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
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
    offset: u64,
    length: usize,
) -> Result<ReadFileResult, RemoteError> {
    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let sftp = open_sftp(ssh).await?;
    let mut file = sftp
        .open(path)
        .await
        .map_err(|e| RemoteError::Sftp(format!("open failed: {e}")))?;

    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| RemoteError::Sftp(format!("seek failed: {e}")))?;
    }

    let cap = length.min(1024 * 1024); // cap at 1MB
    let mut buf = vec![0u8; cap];
    let n = file
        .read(&mut buf)
        .await
        .map_err(|e| RemoteError::Sftp(format!("read failed: {e}")))?;
    buf.truncate(n);

    let data = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(ReadFileResult {
        data,
        bytes_read: n as u64,
    })
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct ReadFileResult {
    pub data: String,
    #[ts(as = "f64")]
    pub bytes_read: u64,
}

/// Write data to a file (base64-encoded input).
pub async fn write_file(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
    data_b64: &str,
) -> Result<u64, RemoteError> {
    use base64::Engine;
    use tokio::io::AsyncWriteExt;

    let data = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| RemoteError::Sftp(format!("invalid base64: {e}")))?;

    let sftp = open_sftp(ssh).await?;
    let mut file = sftp
        .create(path)
        .await
        .map_err(|e| RemoteError::Sftp(format!("create failed: {e}")))?;

    file.write_all(&data)
        .await
        .map_err(|e| RemoteError::Sftp(format!("write failed: {e}")))?;

    Ok(data.len() as u64)
}

/// Create a directory.
pub async fn mkdir(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
) -> Result<(), RemoteError> {
    let sftp = open_sftp(ssh).await?;
    sftp.create_dir(path)
        .await
        .map_err(|e| RemoteError::Sftp(format!("mkdir failed: {e}")))
}

/// Rename a file or directory.
pub async fn rename(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    from: &str,
    to: &str,
) -> Result<(), RemoteError> {
    let sftp = open_sftp(ssh).await?;
    sftp.rename(from, to)
        .await
        .map_err(|e| RemoteError::Sftp(format!("rename failed: {e}")))
}

/// Delete a file or directory.
pub async fn remove(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
    is_dir: bool,
) -> Result<(), RemoteError> {
    let sftp = open_sftp(ssh).await?;
    if is_dir {
        sftp.remove_dir(path)
            .await
            .map_err(|e| RemoteError::Sftp(format!("rmdir failed: {e}")))
    } else {
        sftp.remove_file(path)
            .await
            .map_err(|e| RemoteError::Sftp(format!("remove failed: {e}")))
    }
}

/// Resolve a path to its canonical absolute form.
pub async fn realpath(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    path: &str,
) -> Result<String, RemoteError> {
    let sftp = open_sftp(ssh).await?;
    sftp.canonicalize(path)
        .await
        .map_err(|e| RemoteError::Sftp(format!("realpath failed: {e}")))
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
