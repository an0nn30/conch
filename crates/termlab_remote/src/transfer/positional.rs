//! Async positional file access for the pipelined engine: concurrent chunk
//! tasks read/write at explicit offsets through one shared descriptor, so no
//! task ever depends on a shared cursor. Blocking syscalls run on the tokio
//! blocking pool.

use std::io;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct PositionalFile {
    file: Arc<std::fs::File>,
}

impl PositionalFile {
    pub fn open_read(path: &Path) -> io::Result<Self> {
        Ok(Self { file: Arc::new(std::fs::File::open(path)?) })
    }

    pub fn open_write(path: &Path, truncate: bool) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(truncate)
            .open(path)?;
        Ok(Self { file: Arc::new(file) })
    }

    pub async fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || {
            let mut buffer = vec![0u8; len];
            let read = read_at_impl(&file, &mut buffer, offset)?;
            buffer.truncate(read);
            Ok(buffer)
        })
        .await
        .map_err(|join| io::Error::other(format!("positional read task failed: {join}")))?
    }

    pub async fn write_at(&self, offset: u64, data: Vec<u8>) -> io::Result<()> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || write_at_impl(&file, &data, offset))
            .await
            .map_err(|join| io::Error::other(format!("positional write task failed: {join}")))?
    }

    pub async fn sync(&self) -> io::Result<()> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || file.sync_all())
            .await
            .map_err(|join| io::Error::other(format!("positional sync task failed: {join}")))?
    }
}

#[cfg(unix)]
fn read_at_impl(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, offset)
}

#[cfg(unix)]
fn write_at_impl(file: &std::fs::File, data: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(data, offset)
}

#[cfg(windows)]
fn read_at_impl(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buffer, offset)
}

#[cfg(windows)]
fn write_at_impl(file: &std::fs::File, data: &[u8], offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut written = 0;
    while written < data.len() {
        let n = file.seek_write(&data[written..], offset + written as u64)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "seek_write wrote zero bytes"));
        }
        written += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PositionalFile;

    #[tokio::test]
    async fn out_of_order_writes_then_reads_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("positional.bin");
        let writer = PositionalFile::open_write(&path, true).expect("open write");
        writer.write_at(4, b"5678".to_vec()).await.expect("tail first");
        writer.write_at(0, b"1234".to_vec()).await.expect("head second");
        writer.sync().await.expect("sync");

        let reader = PositionalFile::open_read(&path).expect("open read");
        assert_eq!(reader.read_at(0, 8).await.expect("read all"), b"12345678");
        assert_eq!(reader.read_at(6, 10).await.expect("short tail read"), b"78",
            "reads at EOF return the available bytes");
    }

    #[tokio::test]
    async fn open_write_without_truncate_preserves_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("resume.bin");
        std::fs::write(&path, b"keepme").expect("seed");
        let writer = PositionalFile::open_write(&path, false).expect("open resume");
        writer.write_at(6, b"!".to_vec()).await.expect("append via offset");
        writer.sync().await.expect("sync");
        assert_eq!(std::fs::read(&path).expect("read"), b"keepme!");
    }
}
