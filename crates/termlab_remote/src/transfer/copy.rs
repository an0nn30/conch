use std::io::SeekFrom;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

const MAX_COPY_CHUNK_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDecision {
    Continue,
    Pause,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    Completed { bytes: u64 },
    Paused { bytes: u64 },
    Cancelled { bytes: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyStage {
    SeekSource,
    SeekDestination,
    ReadSource,
    WriteDestination,
}

impl std::fmt::Display for CopyStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SeekSource => "seek source",
            Self::SeekDestination => "seek destination",
            Self::ReadSource => "read source",
            Self::WriteDestination => "write destination",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyError {
    InvalidChunkSize,
    OffsetBeyondSource {
        offset: u64,
        total: u64,
    },
    Io {
        stage: CopyStage,
        kind: std::io::ErrorKind,
        cause: String,
    },
}

impl CopyError {
    fn io(stage: CopyStage, error: std::io::Error) -> Self {
        Self::Io {
            stage,
            kind: error.kind(),
            cause: error.to_string(),
        }
    }
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidChunkSize => {
                formatter.write_str("copy chunk size must be greater than zero")
            }
            Self::OffsetBeyondSource { offset, total } => {
                write!(
                    formatter,
                    "copy offset {offset} exceeds source size {total}"
                )
            }
            Self::Io { stage, cause, .. } => write!(formatter, "{stage} failed: {cause}"),
        }
    }
}

impl std::error::Error for CopyError {}

impl From<CopyError> for crate::error::RemoteError {
    fn from(error: CopyError) -> Self {
        Self::Transfer(error.to_string())
    }
}

/// Copy bounded chunks from an explicit absolute offset.
///
/// `progress` is transient display progress. The returned byte position only
/// becomes a durable resume checkpoint after the caller flushes/syncs the
/// destination; [`super::upload_to_partial`] and [`super::download_to_partial`]
/// perform that finalization before returning.
pub async fn copy_with_checkpoint<R, W, C, P>(
    source: &mut R,
    destination: &mut W,
    offset: u64,
    total: u64,
    chunk_size: usize,
    control: C,
    progress: P,
) -> Result<CopyOutcome, crate::error::RemoteError>
where
    R: AsyncRead + AsyncSeek + Unpin,
    W: AsyncWrite + AsyncSeek + Unpin,
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    copy_with_checkpoint_typed(
        source,
        destination,
        offset,
        total,
        chunk_size,
        control,
        progress,
    )
    .await
    .map_err(Into::into)
}

/// The typed copy boundary used by durable transfer execution.
///
/// Unlike the compatibility wrapper, this preserves whether an I/O failure
/// came from the source or destination before callers add path context.
pub async fn copy_with_checkpoint_typed<R, W, C, P>(
    source: &mut R,
    destination: &mut W,
    offset: u64,
    total: u64,
    chunk_size: usize,
    mut control: C,
    mut progress: P,
) -> Result<CopyOutcome, CopyError>
where
    R: AsyncRead + AsyncSeek + Unpin,
    W: AsyncWrite + AsyncSeek + Unpin,
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    if chunk_size == 0 {
        return Err(CopyError::InvalidChunkSize);
    }
    if offset > total {
        return Err(CopyError::OffsetBeyondSource { offset, total });
    }

    source
        .seek(SeekFrom::Start(offset))
        .await
        .map_err(|error| CopyError::io(CopyStage::SeekSource, error))?;
    destination
        .seek(SeekFrom::Start(offset))
        .await
        .map_err(|error| CopyError::io(CopyStage::SeekDestination, error))?;

    let mut bytes = offset;
    let mut buffer = vec![0; chunk_size.min(MAX_COPY_CHUNK_SIZE)];

    while bytes < total {
        let remaining = total - bytes;
        let read_limit = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = source
            .read(&mut buffer[..read_limit])
            .await
            .map_err(|error| CopyError::io(CopyStage::ReadSource, error))?;
        if read == 0 {
            return Ok(CopyOutcome::Completed { bytes });
        }

        destination
            .write_all(&buffer[..read])
            .await
            .map_err(|error| CopyError::io(CopyStage::WriteDestination, error))?;
        bytes += read as u64;
        progress(bytes, total);

        match control() {
            ControlDecision::Continue => {}
            ControlDecision::Pause => return Ok(CopyOutcome::Paused { bytes }),
            ControlDecision::Cancel => return Ok(CopyOutcome::Cancelled { bytes }),
        }
    }

    Ok(CopyOutcome::Completed { bytes })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Error, ErrorKind, SeekFrom};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};

    use super::{
        ControlDecision, CopyError, CopyOutcome, CopyStage, copy_with_checkpoint,
        copy_with_checkpoint_typed,
    };

    struct CountingReader {
        inner: Cursor<Vec<u8>>,
        reads: usize,
        largest_read_buffer: usize,
    }

    impl AsyncRead for CountingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            self.reads += 1;
            self.largest_read_buffer = self.largest_read_buffer.max(buf.remaining());
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncSeek for CountingReader {
        fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
            Pin::new(&mut self.inner).start_seek(position)
        }

        fn poll_complete(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Pin::new(&mut self.inner).poll_complete(cx)
        }
    }

    struct FailingReader;

    impl AsyncRead for FailingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(Error::new(ErrorKind::Other, "read broke")))
        }
    }

    impl AsyncSeek for FailingReader {
        fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> std::io::Result<()> {
            Ok(())
        }

        fn poll_complete(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(0))
        }
    }

    struct FailingWriter;

    struct ShortWriter {
        inner: Cursor<Vec<u8>>,
        max_write: usize,
        writes: usize,
    }

    impl AsyncWrite for ShortWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.writes += 1;
            let limit = buf.len().min(self.max_write);
            Pin::new(&mut self.inner).poll_write(cx, &buf[..limit])
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    impl AsyncSeek for ShortWriter {
        fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
            Pin::new(&mut self.inner).start_seek(position)
        }

        fn poll_complete(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Pin::new(&mut self.inner).poll_complete(cx)
        }
    }

    impl AsyncWrite for FailingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(Error::new(ErrorKind::Other, "write broke")))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncSeek for FailingWriter {
        fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> std::io::Result<()> {
            Ok(())
        }

        fn poll_complete(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(0))
        }
    }

    #[tokio::test]
    async fn resume_seeks_both_streams_and_reports_absolute_checkpoint() {
        let mut source = Cursor::new(b"0123456789".to_vec());
        let mut destination = Cursor::new(b"0123xxxxxx".to_vec());
        let mut seen = Vec::new();

        let outcome = copy_with_checkpoint(
            &mut source,
            &mut destination,
            4,
            10,
            2,
            || ControlDecision::Continue,
            |done, total| seen.push((done, total)),
        )
        .await
        .unwrap();

        assert_eq!(outcome, CopyOutcome::Completed { bytes: 10 });
        assert_eq!(destination.into_inner(), b"0123456789");
        assert_eq!(seen.last(), Some(&(10, 10)));
    }

    #[tokio::test]
    async fn pause_returns_after_the_current_chunk_checkpoint() {
        let mut source = Cursor::new(b"012345".to_vec());
        let mut destination = Cursor::new(Vec::new());
        let mut seen = Vec::new();

        let outcome = copy_with_checkpoint(
            &mut source,
            &mut destination,
            0,
            6,
            2,
            || ControlDecision::Pause,
            |done, total| seen.push((done, total)),
        )
        .await
        .unwrap();

        assert_eq!(outcome, CopyOutcome::Paused { bytes: 2 });
        assert_eq!(destination.into_inner(), b"01");
        assert_eq!(seen, vec![(2, 6)]);
    }

    #[tokio::test]
    async fn cancel_does_not_read_a_later_chunk() {
        let mut source = CountingReader {
            inner: Cursor::new(b"012345".to_vec()),
            reads: 0,
            largest_read_buffer: 0,
        };
        let mut destination = Cursor::new(Vec::new());

        let outcome = copy_with_checkpoint(
            &mut source,
            &mut destination,
            0,
            6,
            2,
            || ControlDecision::Cancel,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(outcome, CopyOutcome::Cancelled { bytes: 2 });
        assert_eq!(source.reads, 1);
        assert_eq!(destination.into_inner(), b"01");
    }

    #[tokio::test]
    async fn caller_chunk_size_is_capped_to_a_bounded_allocation() {
        let mut source = CountingReader {
            inner: Cursor::new(b"0".to_vec()),
            reads: 0,
            largest_read_buffer: 0,
        };
        let mut destination = Cursor::new(Vec::new());

        let outcome = copy_with_checkpoint(
            &mut source,
            &mut destination,
            0,
            1_000_000,
            1_000_000,
            || ControlDecision::Pause,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(outcome, CopyOutcome::Paused { bytes: 1 });
        assert!(source.largest_read_buffer <= 256 * 1024);
    }

    #[tokio::test]
    async fn eof_completes_with_the_actual_absolute_byte_count() {
        let mut source = Cursor::new(b"012".to_vec());
        let mut destination = Cursor::new(Vec::new());

        let outcome = copy_with_checkpoint(
            &mut source,
            &mut destination,
            0,
            10,
            4,
            || ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(outcome, CopyOutcome::Completed { bytes: 3 });
        assert_eq!(destination.into_inner(), b"012");
    }

    #[tokio::test]
    async fn read_errors_preserve_typed_source_provenance() {
        let mut source = FailingReader;
        let mut destination = Cursor::new(Vec::new());

        let error = copy_with_checkpoint_typed(
            &mut source,
            &mut destination,
            0,
            1,
            1,
            || ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            CopyError::Io {
                stage: CopyStage::ReadSource,
                kind: ErrorKind::Other,
                cause,
            } if cause == "read broke"
        ));
    }

    #[tokio::test]
    async fn write_errors_preserve_typed_destination_provenance() {
        let mut source = Cursor::new(b"0".to_vec());
        let mut destination = FailingWriter;

        let error = copy_with_checkpoint_typed(
            &mut source,
            &mut destination,
            0,
            1,
            1,
            || ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            CopyError::Io {
                stage: CopyStage::WriteDestination,
                kind: ErrorKind::Other,
                cause,
            } if cause == "write broke"
        ));
    }

    #[tokio::test]
    async fn short_destination_writes_are_retried_until_the_whole_chunk_is_written() {
        let mut source = Cursor::new(b"0123456789".to_vec());
        let mut destination = ShortWriter {
            inner: Cursor::new(Vec::new()),
            max_write: 2,
            writes: 0,
        };

        let outcome = copy_with_checkpoint_typed(
            &mut source,
            &mut destination,
            0,
            10,
            10,
            || ControlDecision::Continue,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(outcome, CopyOutcome::Completed { bytes: 10 });
        assert_eq!(destination.inner.into_inner(), b"0123456789");
        assert_eq!(destination.writes, 5);
    }
}
