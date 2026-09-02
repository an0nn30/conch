//! Async positional file access for the pipelined engine: concurrent chunk
//! tasks read/write at explicit offsets through one shared descriptor, so no
//! task ever depends on a shared cursor. Blocking syscalls run on the tokio
//! blocking pool.

use std::io;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct PositionalFile {
    file: Arc<std::fs::File>,
}

impl PositionalFile {
    pub(crate) fn open_read(path: &Path) -> io::Result<Self> {
        Ok(Self {
            file: Arc::new(std::fs::File::open(path)?),
        })
    }

    pub(crate) fn open_write(path: &Path, truncate: bool) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(truncate)
            .open(path)?;
        Ok(Self {
            file: Arc::new(file),
        })
    }

    pub(crate) async fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || {
            let mut buffer = vec![0u8; len];
            let read = read_full_at(&file, &mut buffer, offset)?;
            buffer.truncate(read);
            Ok(buffer)
        })
        .await
        .map_err(|join| io::Error::other(format!("positional read task failed: {join}")))?
    }

    pub(crate) async fn write_at(&self, offset: u64, data: Vec<u8>) -> io::Result<()> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || write_at_impl(&file, &data, offset))
            .await
            .map_err(|join| io::Error::other(format!("positional write task failed: {join}")))?
    }

    pub(crate) async fn sync(&self) -> io::Result<()> {
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || file.sync_all())
            .await
            .map_err(|join| io::Error::other(format!("positional sync task failed: {join}")))?
    }
}

/// Fill `buffer` from `offset`, re-issuing positional reads for whatever the
/// OS did not hand back in one call.
///
/// `pread`/`seek_read` are both allowed to return fewer bytes than requested
/// mid-file — signals, large requests, and network-backed filesystems all do
/// it. The pipelined engine treats a non-tail short read as `UnexpectedEof`
/// (see `pipelined::transfer_chunk`), so a legal short read here would fail
/// the whole chunk. Looping keeps the engine's tail logic the sole EOF
/// authority: a returned length below `buffer.len()` now means genuine EOF.
fn read_full_at(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    read_full_with(|slice, at| read_at_impl(file, slice, at), buffer, offset)
}

/// The loop behind [`read_full_at`], with the positional read injected so it
/// can be unit-tested against a short-returning reader (real `pread` short
/// returns are not reliably forceable from a test).
fn read_full_with<R>(mut read_at: R, buffer: &mut [u8], offset: u64) -> io::Result<usize>
where
    R: FnMut(&mut [u8], u64) -> io::Result<usize>,
{
    let mut filled = 0usize;
    while filled < buffer.len() {
        match read_at(&mut buffer[filled..], offset + filled as u64) {
            Ok(0) => break, // EOF: the caller's truncate reports the real length.
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
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
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "seek_write wrote zero bytes",
            ));
        }
        written += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PositionalFile, read_full_with};
    use std::io;

    #[test]
    fn a_short_positional_read_is_retried_until_the_buffer_is_full() {
        // A reader that never returns more than 3 bytes at a time, exactly the
        // legal-but-short behavior `pread` is allowed to exhibit mid-file.
        let source = b"abcdefghij";
        let mut calls = Vec::new();
        let mut buffer = [0u8; 10];
        let read = read_full_with(
            |slice, at| {
                calls.push((at, slice.len()));
                let take = slice.len().min(3);
                slice[..take].copy_from_slice(&source[at as usize..at as usize + take]);
                Ok(take)
            },
            &mut buffer,
            0,
        )
        .expect("short reads are retried, not failed");
        assert_eq!(read, 10, "the loop must fill the whole buffer");
        assert_eq!(&buffer, source, "retried reads land at the right offsets");
        assert_eq!(
            calls,
            vec![(0, 10), (3, 7), (6, 4), (9, 1)],
            "each follow-up read resumes at offset + already-read, for the remainder"
        );
    }

    #[test]
    fn a_zero_length_read_ends_the_loop_at_eof() {
        let mut buffer = [0u8; 8];
        let read = read_full_with(
            |slice, at| {
                if at >= 4 {
                    return Ok(0);
                }
                slice[..4].copy_from_slice(b"1234");
                Ok(4)
            },
            &mut buffer,
            0,
        )
        .expect("EOF is not an error here");
        assert_eq!(
            read, 4,
            "EOF stops the loop and reports the bytes actually read"
        );
    }

    #[test]
    fn an_interrupted_read_is_retried_and_a_real_error_propagates() {
        let mut interrupted = true;
        let mut buffer = [0u8; 4];
        let read = read_full_with(
            |slice, _at| {
                if std::mem::take(&mut interrupted) {
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                slice.copy_from_slice(b"wxyz");
                Ok(4)
            },
            &mut buffer,
            0,
        )
        .expect("EINTR is retried");
        assert_eq!(read, 4);
        assert_eq!(&buffer, b"wxyz");

        let mut buffer = [0u8; 4];
        let error = read_full_with(
            |_slice, _at| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            &mut buffer,
            0,
        )
        .expect_err("a real error is not swallowed");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn out_of_order_writes_then_reads_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("positional.bin");
        let writer = PositionalFile::open_write(&path, true).expect("open write");
        writer
            .write_at(4, b"5678".to_vec())
            .await
            .expect("tail first");
        writer
            .write_at(0, b"1234".to_vec())
            .await
            .expect("head second");
        writer.sync().await.expect("sync");

        let reader = PositionalFile::open_read(&path).expect("open read");
        assert_eq!(reader.read_at(0, 8).await.expect("read all"), b"12345678");
        assert_eq!(
            reader.read_at(6, 10).await.expect("short tail read"),
            b"78",
            "reads at EOF return the available bytes"
        );
    }

    #[tokio::test]
    async fn open_write_without_truncate_preserves_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("resume.bin");
        std::fs::write(&path, b"keepme").expect("seed");
        let writer = PositionalFile::open_write(&path, false).expect("open resume");
        writer
            .write_at(6, b"!".to_vec())
            .await
            .expect("append via offset");
        writer.sync().await.expect("sync");
        assert_eq!(std::fs::read(&path).expect("read"), b"keepme!");
    }
}
