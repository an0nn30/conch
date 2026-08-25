//! Windowed chunk scheduler: keeps up to `depth` offset-addressed chunk
//! transfers in flight and reports only the contiguous frontier (see
//! docs/superpowers/specs/2026-08-25-sftp-pipelined-transfers-design.md).

use futures::stream::{FuturesUnordered, StreamExt};

use super::copy::{ControlDecision, CopyError, CopyOutcome, CopyStage};
use super::frontier::Frontier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineTuning {
    pub depth: usize,
    pub chunk_bytes: usize,
}

#[async_trait::async_trait]
pub trait ChunkSource: Send + Sync {
    /// Read up to `len` bytes at `offset`. Short reads are only legal at EOF.
    async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, std::io::Error>;
}

#[async_trait::async_trait]
pub trait ChunkSink: Send + Sync {
    async fn write_at(&self, offset: u64, data: Vec<u8>) -> Result<(), std::io::Error>;
}

async fn transfer_chunk(
    source: &dyn ChunkSource,
    sink: &dyn ChunkSink,
    offset: u64,
    len: usize,
    is_tail: bool,
) -> Result<(u64, u64), CopyError> {
    let data = source
        .read_at(offset, len)
        .await
        .map_err(|error| CopyError::Io {
            stage: CopyStage::ReadSource,
            kind: error.kind(),
            cause: error.to_string(),
        })?;
    if data.len() < len && !is_tail {
        return Err(CopyError::Io {
            stage: CopyStage::ReadSource,
            kind: std::io::ErrorKind::UnexpectedEof,
            cause: format!("short read of {} bytes at offset {offset}", data.len()),
        });
    }
    let written = data.len() as u64;
    sink.write_at(offset, data)
        .await
        .map_err(|error| CopyError::Io {
            stage: CopyStage::WriteDestination,
            kind: error.kind(),
            cause: error.to_string(),
        })?;
    Ok((offset, written))
}

/// Keep up to `tuning.depth` chunk transfers in flight and report only the
/// contiguous frontier of durable bytes.
///
/// Depth 1 issues strictly sequential requests (used to pin ordered
/// scheduling behavior in tests and to support servers/paths that cannot
/// tolerate concurrent requests). A failure or a `Pause`/`Cancel` decision
/// stops issuing new work and drains the requests already in flight — bounded
/// by `depth` — before returning, so no task outlives the call.
pub async fn pipelined_copy<C, P>(
    source: &dyn ChunkSource,
    sink: &dyn ChunkSink,
    offset: u64,
    total: u64,
    tuning: PipelineTuning,
    mut control: C,
    mut progress: P,
) -> Result<CopyOutcome, CopyError>
where
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    if tuning.depth == 0 || tuning.chunk_bytes == 0 {
        return Err(CopyError::InvalidChunkSize);
    }
    if offset > total {
        return Err(CopyError::OffsetBeyondSource { offset, total });
    }

    let mut frontier = Frontier::new(offset);
    let mut next_offset = offset;
    let mut in_flight = FuturesUnordered::new();
    let mut failure: Option<CopyError> = None;
    let mut stop: Option<ControlDecision> = None;

    let issue = |at: u64| {
        let len = (total - at).min(tuning.chunk_bytes as u64) as usize;
        let is_tail = at + len as u64 >= total;
        transfer_chunk(source, sink, at, len, is_tail)
    };

    // Prime the window.
    while next_offset < total && in_flight.len() < tuning.depth {
        in_flight.push(issue(next_offset));
        next_offset = (next_offset + tuning.chunk_bytes as u64).min(total);
    }

    while let Some(completed) = in_flight.next().await {
        match completed {
            Ok((at, len)) => {
                let position = frontier.complete(at, len);
                progress(position, total);
            }
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
        if failure.is_none() && stop.is_none() {
            match control() {
                ControlDecision::Continue => {}
                decision => stop = Some(decision),
            }
        }
        // Keep the window full only while healthy and not stopping.
        while failure.is_none()
            && stop.is_none()
            && next_offset < total
            && in_flight.len() < tuning.depth
        {
            in_flight.push(issue(next_offset));
            next_offset = (next_offset + tuning.chunk_bytes as u64).min(total);
        }
        // A failure or stop drains the remaining in-flight chunks (bounded by
        // the window) so no task outlives the call.
    }

    if let Some(error) = failure {
        return Err(error);
    }
    let bytes = frontier.position();
    match stop {
        Some(ControlDecision::Pause) => Ok(CopyOutcome::Paused { bytes }),
        Some(ControlDecision::Cancel) => Ok(CopyOutcome::Cancelled { bytes }),
        _ => Ok(CopyOutcome::Completed { bytes }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::copy::{ControlDecision, CopyOutcome, CopyStage};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Tracks concurrent in-flight chunk transfers, shared between a
    /// `ScriptedSource` and `CollectingSink` pair via `Arc`, so a test can
    /// observe the actual maximum window width the scheduler opens rather
    /// than inferring it indirectly from issue order.
    struct InFlight {
        current: AtomicUsize,
        max: AtomicUsize,
    }

    impl InFlight {
        fn new() -> Self {
            Self { current: AtomicUsize::new(0), max: AtomicUsize::new(0) }
        }

        /// Call when a chunk transfer starts (read issued).
        fn enter(&self) {
            let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(now, Ordering::SeqCst);
        }

        /// Call when a chunk transfer ends (write completed, or aborted).
        fn exit(&self) {
            self.current.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Chunk source over an in-memory buffer. Every read takes at least one
    /// cooperative yield before completing so concurrently issued reads can
    /// interleave under the test runtime. When `stagger` is set to
    /// `(chunk_bytes, depth)`, a read at `offset` takes
    /// `(offset / chunk_bytes) % depth` *additional* yields, so within a
    /// depth-sized batch of concurrently issued reads, later offsets resolve
    /// slower — and once the window slides, a fresh low-stagger offset from
    /// the next batch races ahead of an older high-stagger one still
    /// pending. That produces genuine out-of-order completions, exercising
    /// the scheduler's frontier gap-handling instead of merely completing in
    /// issue order.
    struct ScriptedSource {
        data: Vec<u8>,
        issued: Mutex<Vec<u64>>,
        fail_at: Option<u64>,
        in_flight: Arc<InFlight>,
        stagger: Option<(u64, usize)>,
    }

    #[async_trait::async_trait]
    impl ChunkSource for ScriptedSource {
        async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, std::io::Error> {
            self.issued.lock().unwrap().push(offset);
            self.in_flight.enter();
            if self.fail_at == Some(offset) {
                self.in_flight.exit();
                return Err(std::io::Error::other("scripted read failure"));
            }
            let extra_yields = match self.stagger {
                Some((chunk_bytes, depth)) if depth > 0 => (offset / chunk_bytes) as usize % depth,
                _ => 0,
            };
            for _ in 0..=extra_yields {
                tokio::task::yield_now().await;
            }
            let start = offset as usize;
            let end = (start + len).min(self.data.len());
            Ok(self.data.get(start..end).unwrap_or(&[]).to_vec())
        }
    }

    struct CollectingSink {
        written: Mutex<Vec<(u64, Vec<u8>)>>,
        fail_at: Option<u64>,
        in_flight: Arc<InFlight>,
    }

    #[async_trait::async_trait]
    impl ChunkSink for CollectingSink {
        async fn write_at(&self, offset: u64, data: Vec<u8>) -> Result<(), std::io::Error> {
            if self.fail_at == Some(offset) {
                self.in_flight.exit();
                return Err(std::io::Error::other("scripted write failure"));
            }
            self.written.lock().unwrap().push((offset, data));
            self.in_flight.exit();
            Ok(())
        }
    }

    fn source_with(bytes: usize, in_flight: Arc<InFlight>, stagger: Option<(u64, usize)>) -> ScriptedSource {
        ScriptedSource {
            data: (0..bytes).map(|i| (i % 251) as u8).collect(),
            issued: Mutex::new(Vec::new()),
            fail_at: None,
            in_flight,
            stagger,
        }
    }

    fn sink_with(in_flight: Arc<InFlight>) -> CollectingSink {
        CollectingSink { written: Mutex::new(Vec::new()), fail_at: None, in_flight }
    }

    fn source(bytes: usize) -> ScriptedSource {
        source_with(bytes, Arc::new(InFlight::new()), None)
    }

    fn sink() -> CollectingSink {
        sink_with(Arc::new(InFlight::new()))
    }

    fn reassemble(sink: &CollectingSink, total: usize) -> Vec<u8> {
        let mut out = vec![0u8; total];
        for (offset, data) in sink.written.lock().unwrap().iter() {
            out[*offset as usize..*offset as usize + data.len()].copy_from_slice(data);
        }
        out
    }

    #[tokio::test]
    async fn copies_everything_and_reports_contiguous_progress() {
        // Stagger completions (see `ScriptedSource` doc comment) so this test
        // actually exercises out-of-order completion, not just the in-order
        // happy path.
        let src = source_with(1000, Arc::new(InFlight::new()), Some((100, 4)));
        let dst = sink();
        let mut reports = Vec::new();
        let outcome = pipelined_copy(
            &src, &dst, 0, 1000,
            PipelineTuning { depth: 4, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |done, total| reports.push((done, total)),
        ).await.expect("copy succeeds");
        assert_eq!(outcome, CopyOutcome::Completed { bytes: 1000 });
        assert_eq!(reassemble(&dst, 1000), src.data);
        assert!(reports.windows(2).all(|w| w[0].0 <= w[1].0), "progress is monotonic");
        assert_eq!(reports.last().copied(), Some((1000, 1000)));
        // With staggered completions, some completion events fill a gap
        // behind the frontier rather than extending it. If the scheduler
        // reported progress in raw completion order instead of frontier
        // order, no two consecutive reports would ever repeat the same
        // `done` value.
        assert!(reports.windows(2).any(|w| w[0].0 == w[1].0),
            "expected at least one completion to be blocked by a gap, proving the frontier — \
             not completion order — gates reported progress");
    }

    #[tokio::test]
    async fn depth_one_issues_strictly_sequential_offsets() {
        let src = source(500);
        let dst = sink();
        pipelined_copy(
            &src, &dst, 0, 500,
            PipelineTuning { depth: 1, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect("copy succeeds");
        assert_eq!(*src.issued.lock().unwrap(), vec![0, 100, 200, 300, 400],
            "depth 1 must be observably sequential");
    }

    #[tokio::test]
    async fn window_never_exceeds_depth() {
        // Share one tracker between source and sink so "in flight" spans the
        // whole read-then-write lifecycle of a chunk, and directly observe
        // the maximum concurrency the scheduler actually opens.
        let tracker = Arc::new(InFlight::new());
        let src = source_with(2000, Arc::clone(&tracker), None);
        let dst = sink_with(Arc::clone(&tracker));
        pipelined_copy(
            &src, &dst, 0, 2000,
            PipelineTuning { depth: 3, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect("copy succeeds");
        let max_in_flight = tracker.max.load(Ordering::SeqCst);
        assert!(max_in_flight <= 3,
            "observed {max_in_flight} chunks concurrently in flight, exceeding window depth 3");
        assert!(max_in_flight > 1,
            "expected real pipelining (more than one chunk in flight at once) at depth 3, \
             observed max concurrency {max_in_flight}");
    }

    #[tokio::test]
    async fn resume_offset_starts_midway() {
        let src = source(1000);
        let dst = sink();
        let outcome = pipelined_copy(
            &src, &dst, 600, 1000,
            PipelineTuning { depth: 4, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect("copy succeeds");
        assert_eq!(outcome, CopyOutcome::Completed { bytes: 1000 });
        let issued = src.issued.lock().unwrap();
        assert!(issued.iter().all(|offset| *offset >= 600), "no chunk below the resume offset");
        assert_eq!(reassemble(&dst, 1000)[600..], src.data[600..]);
    }

    #[tokio::test]
    async fn pause_drains_and_reports_the_frontier() {
        let src = source(10_000);
        let dst = sink();
        let depth = 4;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_control = Arc::clone(&calls);
        let outcome = pipelined_copy(
            &src, &dst, 0, 10_000,
            PipelineTuning { depth, chunk_bytes: 100 },
            move || {
                let n = calls_for_control.fetch_add(1, Ordering::SeqCst) + 1;
                if n > 10 { ControlDecision::Pause } else { ControlDecision::Continue }
            },
            |_, _| {},
        ).await.expect("pause is a success outcome");
        let CopyOutcome::Paused { bytes } = outcome else {
            panic!("expected paused outcome, got {outcome:?}");
        };
        // A scheduler that ignored Pause and kept copying to completion would
        // still pass every assertion below unless we also confirm it
        // actually stopped short of the whole file, and that it did not
        // issue far more reads than the point at which Pause was decided.
        assert!(bytes < 10_000,
            "pause must stop issuing before the whole 10,000-byte file copies, got {bytes} bytes");
        // Every byte up to the reported frontier must actually be in the sink.
        let written = reassemble(&dst, 10_000);
        assert_eq!(written[..bytes as usize], src.data[..bytes as usize],
            "paused frontier only counts contiguous durable bytes");
        let control_calls = calls.load(Ordering::SeqCst);
        let issued_reads = src.issued.lock().unwrap().len();
        assert!(issued_reads <= control_calls + depth,
            "expected at most {} issued reads (control calls {control_calls} + depth {depth}), \
             got {issued_reads}; scheduler kept issuing new work after Pause",
            control_calls + depth);
    }

    #[tokio::test]
    async fn read_failure_aborts_with_source_stage() {
        let mut src = source(1000);
        src.fail_at = Some(500);
        let dst = sink();
        let error = pipelined_copy(
            &src, &dst, 0, 1000,
            PipelineTuning { depth: 4, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect_err("scripted failure surfaces");
        let CopyError::Io { stage, .. } = error else { panic!("expected io error") };
        assert_eq!(stage, CopyStage::ReadSource);
    }

    #[tokio::test]
    async fn write_failure_aborts_with_destination_stage() {
        let src = source(1000);
        let mut dst = sink();
        dst.fail_at = Some(300);
        let error = pipelined_copy(
            &src, &dst, 0, 1000,
            PipelineTuning { depth: 4, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect_err("scripted failure surfaces");
        let CopyError::Io { stage, .. } = error else { panic!("expected io error") };
        assert_eq!(stage, CopyStage::WriteDestination);
    }

    #[tokio::test]
    async fn short_read_before_eof_is_an_error() {
        struct ShortSource;
        #[async_trait::async_trait]
        impl ChunkSource for ShortSource {
            async fn read_at(&self, _offset: u64, _len: usize) -> Result<Vec<u8>, std::io::Error> {
                Ok(vec![1, 2, 3]) // always short
            }
        }
        let dst = sink();
        let error = pipelined_copy(
            &ShortSource, &dst, 0, 1000,
            PipelineTuning { depth: 2, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect_err("short read mid-file is corruption, not EOF");
        let CopyError::Io { stage, kind, .. } = error else { panic!("expected io error") };
        assert_eq!(stage, CopyStage::ReadSource);
        assert_eq!(kind, std::io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn invalid_tuning_is_rejected() {
        let src = source(10);
        let dst = sink();
        let error = pipelined_copy(
            &src, &dst, 0, 10,
            PipelineTuning { depth: 0, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect_err("depth 0 rejected");
        assert_eq!(error, CopyError::InvalidChunkSize);
        let error = pipelined_copy(
            &src, &dst, 20, 10,
            PipelineTuning { depth: 1, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect_err("offset beyond total rejected");
        assert_eq!(error, CopyError::OffsetBeyondSource { offset: 20, total: 10 });
    }
}
