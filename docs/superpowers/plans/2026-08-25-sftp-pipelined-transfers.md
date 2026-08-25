# Pipelined SFTP Transfers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make single-file SFTP transfers approach link capacity by keeping a bounded window of offset-addressed chunk requests in flight, without changing any durable-queue contract.

**Architecture:** A pure contiguous-frontier tracker plus a window scheduler live in `termlab_remote::transfer::pipelined`, speaking to endpoints through positional read/write traits. Real endpoints are a `RawSftpSession` (new channel on the existing authenticated SSH connection, offset-addressed requests) and positional local file I/O. The queue runner gains one seam — a `copy_ranges` method on its `TransferIo` trait with a sequential default — so fakes and every existing behavior test are untouched; the real IO overrides it to run the pipelined engine and falls back to sequential if the first window fails.

**Tech Stack:** Rust (tokio, russh 0.48, russh-sftp 2.1, ts-rs), frontend vanilla JS (tl-dialog), node test scripts.

**Spec:** `docs/superpowers/specs/2026-08-25-sftp-pipelined-transfers-design.md`

## Global Constraints

- Defaults: `pipeline_depth` = 16 (range 1–64), `pipeline_chunk_bytes` = 262144 (range 32768–1048576), clamped to the server's negotiated write/read limit at attempt time.
- In-flight memory per attempt ≤ `pipeline_depth × pipeline_chunk_bytes`.
- The persisted checkpoint and every progress report is the contiguous frontier — never counts bytes past a gap.
- Depth 1 must issue strictly sequential, ordered requests (observably equal to the sequential engine).
- A failure during the FIRST window of a depth>1 attempt retries that attempt once with the sequential engine and a `log::warn!`; later failures are real errors with unchanged typed provenance.
- Persisted settings stay schema v1: new fields are additive with serde defaults; older stores must load.
- Serde uses `rename_all = "camelCase"`; TS types regenerate via the existing ts-rs `#[ts(export)]` flow (`cargo test -p termlab_tauri` exports to `frontend/types/`).
- All work happens on branch `feat/sftp-pipelined-transfers` in the worktree `.worktrees/sftp-durable-transfer-queue-impl`. Commit after every task; never push `--force`; no Co-Authored-By lines.
- Run `cargo test -p termlab_remote` / `-p termlab_tauri` for Rust tasks and `node scripts/tests/test_transfer_center.mjs` (plus the full `scripts/tests` sweep at the end) for frontend tasks.

## File Structure

- Create: `crates/termlab_remote/src/transfer/frontier.rs` — pure contiguous-frontier bookkeeping.
- Create: `crates/termlab_remote/src/transfer/positional.rs` — async positional local-file read/write (unix + windows).
- Create: `crates/termlab_remote/src/transfer/pipelined.rs` — tuning type, endpoint traits, window scheduler.
- Modify: `crates/termlab_remote/src/transfer.rs` — declare the three new modules, re-export public names.
- Modify: `crates/termlab_remote/src/transfer/sftp_io.rs` — raw-session plumbing + pipelined upload/download entry points.
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/model.rs` — `QueueSettings` fields + validation.
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/runner.rs` — `copy_ranges` seam, tuning parameter, fallback.
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/engine.rs` — pass tuning from settings into `run`.
- Modify: `crates/termlab_tauri/frontend/app/features/transfers/dialogs.js` — two new concurrency-dialog fields.
- Test: module `#[cfg(test)]` blocks in each new file, existing runner/engine test modules, `scripts/tests/test_transfer_center.mjs`, `scripts/tests/test_transfer_store.mjs`.

---

### Task 1: Contiguous frontier tracker

**Files:**
- Create: `crates/termlab_remote/src/transfer/frontier.rs`
- Modify: `crates/termlab_remote/src/transfer.rs` (add `pub mod frontier;` next to the existing `pub mod copy;` line)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct Frontier` with `pub fn new(start: u64) -> Frontier`, `pub fn complete(&mut self, offset: u64, len: u64) -> u64` (records a finished chunk, returns the new contiguous frontier), `pub fn position(&self) -> u64`, `pub fn pending(&self) -> usize` (count of recorded-but-not-yet-contiguous ranges). Task 4 drives it; Task 6's checkpoints persist `position()`.

- [ ] **Step 1: Write the failing tests**

At the bottom of the new `frontier.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::Frontier;

    #[test]
    fn in_order_completions_advance_immediately() {
        let mut frontier = Frontier::new(0);
        assert_eq!(frontier.complete(0, 100), 100);
        assert_eq!(frontier.complete(100, 100), 200);
        assert_eq!(frontier.position(), 200);
        assert_eq!(frontier.pending(), 0);
    }

    #[test]
    fn out_of_order_completions_wait_for_the_gap() {
        let mut frontier = Frontier::new(0);
        assert_eq!(frontier.complete(200, 100), 0, "gap at 0..200 blocks the frontier");
        assert_eq!(frontier.complete(100, 100), 0, "gap at 0..100 still blocks");
        assert_eq!(frontier.pending(), 2);
        assert_eq!(frontier.complete(0, 100), 300, "filling the gap folds all pending ranges");
        assert_eq!(frontier.pending(), 0);
    }

    #[test]
    fn resume_start_offset_is_the_floor() {
        let mut frontier = Frontier::new(4096);
        assert_eq!(frontier.position(), 4096);
        assert_eq!(frontier.complete(4096, 512), 4608);
    }

    #[test]
    fn duplicate_and_overlapping_completions_do_not_double_count() {
        let mut frontier = Frontier::new(0);
        frontier.complete(0, 100);
        assert_eq!(frontier.complete(0, 100), 100, "exact duplicate is a no-op");
        assert_eq!(frontier.complete(50, 100), 150, "overlap only extends the uncovered part");
    }

    #[test]
    fn zero_length_completion_is_a_no_op() {
        let mut frontier = Frontier::new(10);
        assert_eq!(frontier.complete(10, 0), 10);
        assert_eq!(frontier.pending(), 0);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p termlab_remote frontier 2>&1 | tail -5`
Expected: compile error — `frontier` module does not exist.

- [ ] **Step 3: Implement**

```rust
//! Contiguous-frontier bookkeeping for out-of-order chunk completion.
//!
//! The durable checkpoint contract (see the pipelined-transfers design doc)
//! is that persisted progress NEVER counts bytes past a gap. This tracker is
//! the single source of that guarantee: `complete()` records finished ranges
//! and only advances `position` across fully covered bytes.

use std::collections::BTreeMap;

#[derive(Debug)]
pub struct Frontier {
    position: u64,
    /// Completed ranges beyond `position`, keyed by start offset.
    pending: BTreeMap<u64, u64>, // start -> end (exclusive)
}

impl Frontier {
    pub fn new(start: u64) -> Self {
        Self { position: start, pending: BTreeMap::new() }
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Record a completed chunk and return the new contiguous frontier.
    pub fn complete(&mut self, offset: u64, len: u64) -> u64 {
        let end = offset.saturating_add(len);
        if end > self.position {
            let start = offset.max(self.position);
            let entry = self.pending.entry(start).or_insert(end);
            if *entry < end {
                *entry = end;
            }
        }
        // Fold every range that now touches the frontier.
        while let Some((&start, &end)) = self.pending.first_key_value() {
            if start > self.position {
                break;
            }
            self.pending.pop_first();
            if end > self.position {
                self.position = end;
            }
        }
        self.position
    }
}
```

Add `pub mod frontier;` to `crates/termlab_remote/src/transfer.rs` beside the existing module declarations.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p termlab_remote frontier 2>&1 | tail -3`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_remote/src/transfer/frontier.rs crates/termlab_remote/src/transfer.rs
git commit -m "Add contiguous frontier tracker for pipelined transfers"
```

---

### Task 2: Queue settings gain pipeline tuning

**Files:**
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/model.rs` (QueueSettings at ~line 353, `validate_queue_settings` at ~line 367, plus the module's existing settings tests)
- Modify (generated): `crates/termlab_tauri/frontend/types/QueueSettings.ts`

**Interfaces:**
- Consumes: existing `QueueSettings { global_limit, per_host_limit }`.
- Produces: `QueueSettings` additionally carrying `pub pipeline_depth: usize` and `pub pipeline_chunk_bytes: usize` with serde defaults (16 / 262144); `validate_queue_settings` enforcing 1–64 and 32768–1048576. Task 6 reads these; Task 7's dialog edits them.

- [ ] **Step 1: Write the failing tests**

In `model.rs`'s existing `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn queue_settings_default_pipeline_tuning() {
    let settings = QueueSettings::default();
    assert_eq!(settings.pipeline_depth, 16);
    assert_eq!(settings.pipeline_chunk_bytes, 256 * 1024);
}

#[test]
fn queue_settings_v1_json_without_pipeline_fields_defaults() {
    let parsed: QueueSettings =
        serde_json::from_str(r#"{"globalLimit":3,"perHostLimit":2}"#)
            .expect("v1 settings without pipeline fields must deserialize");
    assert_eq!(parsed.pipeline_depth, 16);
    assert_eq!(parsed.pipeline_chunk_bytes, 262144);
}

#[test]
fn queue_settings_pipeline_validation_bounds() {
    let mut settings = QueueSettings::default();
    settings.pipeline_depth = 0;
    assert!(validate_queue_settings(&settings).is_err(), "depth 0 rejected");
    settings.pipeline_depth = 65;
    assert!(validate_queue_settings(&settings).is_err(), "depth 65 rejected");
    settings.pipeline_depth = 1;
    settings.pipeline_chunk_bytes = 1024;
    assert!(validate_queue_settings(&settings).is_err(), "tiny chunk rejected");
    settings.pipeline_chunk_bytes = 2 * 1024 * 1024;
    assert!(validate_queue_settings(&settings).is_err(), "huge chunk rejected");
    settings.pipeline_chunk_bytes = 262144;
    assert!(validate_queue_settings(&settings).is_ok());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p termlab_tauri queue_settings 2>&1 | tail -5`
Expected: compile error — no `pipeline_depth` field.

- [ ] **Step 3: Implement**

```rust
fn default_pipeline_depth() -> usize {
    16
}

fn default_pipeline_chunk_bytes() -> usize {
    256 * 1024
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueSettings {
    pub global_limit: usize,
    pub per_host_limit: usize,
    /// Chunk requests kept in flight per transfer attempt (1 = sequential).
    #[serde(default = "default_pipeline_depth")]
    pub pipeline_depth: usize,
    /// Requested bytes per chunk; clamped to the server limit at attempt time.
    #[serde(default = "default_pipeline_chunk_bytes")]
    pub pipeline_chunk_bytes: usize,
}

impl Default for QueueSettings {
    fn default() -> Self {
        Self {
            global_limit: 3,
            per_host_limit: 2,
            pipeline_depth: default_pipeline_depth(),
            pipeline_chunk_bytes: default_pipeline_chunk_bytes(),
        }
    }
}
```

Extend `validate_queue_settings` after the existing two checks:

```rust
    if !(1..=64).contains(&settings.pipeline_depth) {
        return Err("pipeline depth must be between 1 and 64".into());
    }
    if !(32 * 1024..=1024 * 1024).contains(&settings.pipeline_chunk_bytes) {
        return Err("pipeline chunk size must be between 32 KiB and 1 MiB".into());
    }
```

Fix every existing struct literal of `QueueSettings { global_limit, per_host_limit }` in the workspace (compiler will list them; use `..QueueSettings::default()` where natural).

- [ ] **Step 4: Run tests and regenerate the TS type**

Run: `cargo test -p termlab_tauri 2>&1 | grep -E "test result|error" | head`
Expected: all green (ts-rs regenerates `frontend/types/QueueSettings.ts` during the test run — confirm it now contains `pipelineDepth: number, pipelineChunkBytes: number`).

Also run: `node scripts/tests/test_transfer_store.mjs` — must stay green (frontend store passes settings through untyped).

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/remote/transfer_queue/model.rs crates/termlab_tauri/frontend/types/QueueSettings.ts
git commit -m "Add pipeline depth and chunk size to queue settings"
```

---

### Task 3: Positional local file I/O

**Files:**
- Create: `crates/termlab_remote/src/transfer/positional.rs`
- Modify: `crates/termlab_remote/src/transfer.rs` (add `pub mod positional;`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct PositionalFile` with `pub fn open_read(path: &Path) -> io::Result<PositionalFile>`, `pub fn open_write(path: &Path, truncate: bool) -> io::Result<PositionalFile>`, `pub async fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>>`, `pub async fn write_at(&self, offset: u64, data: Vec<u8>) -> io::Result<()>`, `pub async fn sync(&self) -> io::Result<()>`. Cloneable (`Arc<std::fs::File>` inside) so concurrent chunk tasks share it. Task 5 wraps it in engine endpoints.

- [ ] **Step 1: Write the failing tests**

```rust
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
```

(`tempfile` is already a dev-dependency of `termlab_remote`; if not, add `tempfile = "3"` to its `[dev-dependencies]`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p termlab_remote positional 2>&1 | tail -5`
Expected: compile error — module missing.

- [ ] **Step 3: Implement**

```rust
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
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p termlab_remote positional 2>&1 | tail -3`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_remote/src/transfer/positional.rs crates/termlab_remote/src/transfer.rs crates/termlab_remote/Cargo.toml
git commit -m "Add positional local file IO for pipelined transfers"
```

---

### Task 4: The window scheduler

**Files:**
- Create: `crates/termlab_remote/src/transfer/pipelined.rs`
- Modify: `crates/termlab_remote/src/transfer.rs` (add `pub mod pipelined;`)

**Interfaces:**
- Consumes: `Frontier` (Task 1); `ControlDecision`, `CopyOutcome`, `CopyError`, `CopyStage` from `super::copy`.
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineTuning {
    pub depth: usize,       // >= 1
    pub chunk_bytes: usize, // > 0, pre-clamped by the caller
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

pub async fn pipelined_copy<C, P>(
    source: &dyn ChunkSource,
    sink: &dyn ChunkSink,
    offset: u64,
    total: u64,
    tuning: PipelineTuning,
    control: C,
    progress: P,
) -> Result<CopyOutcome, CopyError>
where
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64);
```

Task 5 implements the traits for real endpoints; Task 6 calls `pipelined_copy`.

- [ ] **Step 1: Write the failing tests**

At the bottom of the new `pipelined.rs`. The fake endpoints record issue order and can reorder/fail completions deterministically:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::copy::{ControlDecision, CopyOutcome, CopyStage};
    use std::sync::Mutex;

    /// Chunk source over an in-memory buffer that releases completions in a
    /// scripted order: each read waits until every earlier-scripted read has
    /// been issued, then completes in the scripted sequence.
    struct ScriptedSource {
        data: Vec<u8>,
        issued: Mutex<Vec<u64>>,
        fail_at: Option<u64>,
    }

    #[async_trait::async_trait]
    impl ChunkSource for ScriptedSource {
        async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, std::io::Error> {
            self.issued.lock().unwrap().push(offset);
            if self.fail_at == Some(offset) {
                return Err(std::io::Error::other("scripted read failure"));
            }
            // Yield so concurrently issued reads interleave under the test runtime.
            tokio::task::yield_now().await;
            let start = offset as usize;
            let end = (start + len).min(self.data.len());
            Ok(self.data.get(start..end).unwrap_or(&[]).to_vec())
        }
    }

    struct CollectingSink {
        written: Mutex<Vec<(u64, Vec<u8>)>>,
        fail_at: Option<u64>,
    }

    #[async_trait::async_trait]
    impl ChunkSink for CollectingSink {
        async fn write_at(&self, offset: u64, data: Vec<u8>) -> Result<(), std::io::Error> {
            if self.fail_at == Some(offset) {
                return Err(std::io::Error::other("scripted write failure"));
            }
            self.written.lock().unwrap().push((offset, data));
            Ok(())
        }
    }

    fn source(bytes: usize) -> ScriptedSource {
        ScriptedSource {
            data: (0..bytes).map(|i| (i % 251) as u8).collect(),
            issued: Mutex::new(Vec::new()),
            fail_at: None,
        }
    }

    fn sink() -> CollectingSink {
        CollectingSink { written: Mutex::new(Vec::new()), fail_at: None }
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
        let src = source(1000);
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
        let src = source(2000);
        let dst = sink();
        pipelined_copy(
            &src, &dst, 0, 2000,
            PipelineTuning { depth: 3, chunk_bytes: 100 },
            || ControlDecision::Continue,
            |_, _| {},
        ).await.expect("copy succeeds");
        // With depth 3, offset N may only be issued after offset N-3 completed:
        // the recorded issue order can never contain an offset more than
        // 3 chunks ahead of the count of chunks issued before it.
        let issued = src.issued.lock().unwrap();
        for (index, offset) in issued.iter().enumerate() {
            assert!(*offset as usize <= (index + 1) * 100 + 200,
                "offset {offset} issued at position {index} exceeds window depth 3");
        }
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
        let mut calls = 0;
        let outcome = pipelined_copy(
            &src, &dst, 0, 10_000,
            PipelineTuning { depth: 4, chunk_bytes: 100 },
            move || {
                calls += 1;
                if calls > 10 { ControlDecision::Pause } else { ControlDecision::Continue }
            },
            |_, _| {},
        ).await.expect("pause is a success outcome");
        let CopyOutcome::Paused { bytes } = outcome else {
            panic!("expected paused outcome, got {outcome:?}");
        };
        // Every byte up to the reported frontier must actually be in the sink.
        let written = reassemble(&dst, 10_000);
        assert_eq!(written[..bytes as usize], src.data[..bytes as usize],
            "paused frontier only counts contiguous durable bytes");
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p termlab_remote pipelined 2>&1 | tail -5`
Expected: compile error — module missing.

- [ ] **Step 3: Implement the scheduler**

```rust
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
```

Add `futures = "0.3"` to `termlab_remote`'s `[dependencies]` if it is not already there (check first: `grep futures crates/termlab_remote/Cargo.toml`), and `pub mod pipelined;` to `transfer.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p termlab_remote pipelined 2>&1 | tail -3`
Expected: 9 passed. Also run the crate's full suite: `cargo test -p termlab_remote 2>&1 | grep "test result"` — no regressions.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_remote/src/transfer/pipelined.rs crates/termlab_remote/src/transfer.rs crates/termlab_remote/Cargo.toml
git commit -m "Add windowed pipelined copy scheduler"
```

---

### Task 5: Real endpoints — raw SFTP session and entry points

**Files:**
- Modify: `crates/termlab_remote/src/transfer/sftp_io.rs`

**Interfaces:**
- Consumes: `RawSftpSession` (russh-sftp), `PositionalFile` (Task 3), `pipelined_copy`/`ChunkSource`/`ChunkSink`/`PipelineTuning` (Task 4), the module's existing `fingerprint_open_local`/`fingerprint_open_remote`, `OpenFlags`, `open_sftp` channel pattern from `crate::sftp`.
- Produces:

```rust
pub async fn upload_to_partial_pipelined<F, Fut, C, P>(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    local_path: impl AsRef<Path>,
    remote_partial_path: &str,
    offset: u64,
    tuning: PipelineTuning,
    on_fingerprint: F,
    control: C,
    progress: P,
) -> Result<CopyOutcome, RemoteError>;

pub async fn download_to_partial_pipelined<F, Fut, C, P>(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    remote_path: &str,
    local_partial_path: impl AsRef<Path>,
    offset: u64,
    tuning: PipelineTuning,
    on_fingerprint: F,
    control: C,
    progress: P,
) -> Result<CopyOutcome, RemoteError>;
```

Same generic bounds as the existing `upload_to_partial` / `download_to_partial` (`F: FnOnce(SourceFingerprint) -> Fut`, `Fut: Future<Output = Result<(), RemoteError>>`, `C: FnMut() -> ControlDecision`, `P: FnMut(u64, u64)`). Task 6 calls these.

- [ ] **Step 1: Read the neighboring code**

Read `crates/termlab_remote/src/sftp.rs` `open_sftp()` (channel-open + `request_subsystem("sftp")` + `SftpSession::new(channel.into_stream())`) and `sftp_io.rs` `open_remote_partial` (the `OpenFlags` used for resume vs fresh). The raw session below mirrors both exactly.

- [ ] **Step 2: Write the failing test**

The raw-session path needs a live server, so the automated test targets the seam that is testable: fingerprint-revalidation and flag selection. Add to `sftp_io.rs`'s test module:

```rust
#[test]
fn pipelined_open_flags_match_sequential_partial_flags() {
    assert_eq!(
        pipelined_remote_open_flags(true),
        resume_partial_open_flags(),
        "resume must not truncate the partial",
    );
    assert_eq!(
        pipelined_remote_open_flags(false),
        fresh_partial_open_flags(),
        "fresh transfers must truncate",
    );
}

#[tokio::test]
async fn pipelined_upload_rejects_source_changed_since_fingerprint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("source.bin");
    std::fs::write(&path, b"original").expect("seed");
    let stale = fingerprint_local_parts(999, None); // wrong size on purpose
    let error = revalidate_local_fingerprint(&path, &stale)
        .await
        .expect_err("changed source must be rejected");
    assert!(matches!(error, RemoteError::Transfer(_)));
    let live = fingerprint_open_local(&path).await.expect("fingerprint").1;
    revalidate_local_fingerprint(&path, &live)
        .await
        .expect("matching fingerprint passes");
}
```

Where `resume_partial_open_flags()` / `fresh_partial_open_flags()` are tiny helpers extracted from the existing `open_remote_partial` flag expressions (pure refactor: `open_remote_partial` must call them so the equality is meaningful, not a copy).

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p termlab_remote sftp_io 2>&1 | tail -5`
Expected: compile error — helpers missing.

- [ ] **Step 4: Implement**

1. Extract the flag helpers from `open_remote_partial`'s existing expressions and make `open_remote_partial` use them:

```rust
pub(crate) fn resume_partial_open_flags() -> OpenFlags {
    OpenFlags::CREATE | OpenFlags::WRITE
}

pub(crate) fn fresh_partial_open_flags() -> OpenFlags {
    OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE
}

pub(crate) fn pipelined_remote_open_flags(resume: bool) -> OpenFlags {
    if resume { resume_partial_open_flags() } else { fresh_partial_open_flags() }
}
```

(Copy the EXACT flag expressions currently in `open_remote_partial` — if they differ from the above, the existing code wins; the helpers exist so both paths share one truth.)

2. Fingerprint revalidation (closes the reopen TOCTOU window — the pipelined path opens its own handles after the runner's Checking phase):

```rust
pub(crate) async fn revalidate_local_fingerprint(
    path: &Path,
    expected: &SourceFingerprint,
) -> Result<(), RemoteError> {
    let (_file, current) = fingerprint_open_local(path).await?;
    if &current != expected {
        return Err(RemoteError::Transfer(format!(
            "local source {} changed since it was checked (size {} -> {})",
            path.display(), expected.size, current.size,
        )));
    }
    Ok(())
}
```

And the remote twin `revalidate_remote_fingerprint(sftp: &SftpSessionHandle, path: &str, expected: &SourceFingerprint)` using `fingerprint_open_remote`.

3. Raw session + handle plumbing (mirrors `open_sftp`):

```rust
use russh_sftp::client::RawSftpSession;

pub(crate) async fn open_raw_sftp_session(
    ssh: &russh::client::Handle<TermLabSshHandler>,
) -> Result<RawSftpSession, RemoteError> {
    let channel = ssh
        .channel_open_session()
        .await
        .map_err(|error| RemoteError::Transfer(format!("open pipelined channel failed: {error}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| RemoteError::Transfer(format!("request sftp subsystem failed: {error}")))?;
    let raw = RawSftpSession::new(channel.into_stream());
    raw.init()
        .await
        .map_err(|error| RemoteError::Transfer(format!("sftp init failed: {error}")))?;
    Ok(raw)
}
```

(If `RawSftpSession` exposes `limits()`/`set_limits` the same way `SftpSession::new_opts` uses them — see `~/.cargo/registry/.../russh-sftp-2.1.1/src/client/session.rs:73` — negotiate limits after `init()` and clamp `tuning.chunk_bytes` to the negotiated `write_len`/`read_len` when present. If the API differs, clamp to 255 KiB, OpenSSH's conventional cap, and leave a comment.)

4. `ChunkSource`/`ChunkSink` adapters:

```rust
struct RawRemoteChunkFile {
    session: RawSftpSession,
    handle: String,
}

#[async_trait]
impl crate::transfer::pipelined::ChunkSource for RawRemoteChunkFile {
    async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, std::io::Error> {
        self.session
            .read(self.handle.as_str(), offset, len as u32)
            .await
            .map(|data| data.data)
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

#[async_trait]
impl crate::transfer::pipelined::ChunkSink for RawRemoteChunkFile {
    async fn write_at(&self, offset: u64, data: Vec<u8>) -> Result<(), std::io::Error> {
        self.session
            .write(self.handle.as_str(), offset, data)
            .await
            .map(|_| ())
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}
```

Plus the trivial impls of both traits for `PositionalFile` (delegate to `read_at`/`write_at`).

(`Data`'s payload field name is `data` per the russh-sftp protocol types; `cargo check` confirms. SFTP `read` returns EOF as an error status on some servers — map a `StatusCode::Eof` error to `Ok(Vec::new())` so the engine's short-read/tail logic owns EOF handling.)

5. The two public entry points. Upload shape (download is the mirror image — remote raw read source via `RawRemoteChunkFile` opened read-only with `OpenFlags::READ`, local `PositionalFile::open_write(path, offset == 0)` sink):

```rust
pub async fn upload_to_partial_pipelined<F, Fut, C, P>(
    ssh: &russh::client::Handle<TermLabSshHandler>,
    local_path: impl AsRef<Path>,
    remote_partial_path: &str,
    offset: u64,
    tuning: PipelineTuning,
    on_fingerprint: F,
    control: C,
    progress: P,
) -> Result<CopyOutcome, RemoteError>
where
    F: FnOnce(SourceFingerprint) -> Fut,
    Fut: Future<Output = Result<(), RemoteError>>,
    C: FnMut() -> ControlDecision,
    P: FnMut(u64, u64),
{
    let local_path = local_path.as_ref();
    let (_stream, fingerprint) = fingerprint_open_local(local_path).await?;
    let total = fingerprint.size;
    on_fingerprint(fingerprint).await?;

    let source = PositionalFile::open_read(local_path)
        .map_err(|error| RemoteError::Transfer(format!(
            "open local source {} for pipelined upload failed: {error}", local_path.display())))?;

    let raw = open_raw_sftp_session(ssh).await?;
    let opened = raw
        .open(remote_partial_path, pipelined_remote_open_flags(offset > 0), FileAttributes::default())
        .await
        .map_err(|error| RemoteError::Transfer(format!(
            "open remote partial {remote_partial_path} failed: {error}")))?;
    let sink = RawRemoteChunkFile { session: raw, handle: opened.handle };

    let copy_result = pipelined_copy(&source, &sink, offset, total, tuning, control, progress)
        .await
        .map_err(RemoteError::from);
    let close_result = sink
        .session
        .close(sink.handle.as_str())
        .await
        .map(|_| ())
        .map_err(|error| RemoteError::Transfer(format!("close remote partial failed: {error}")));
    match (copy_result, close_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Ok(_), Err(close_error)) => Err(close_error),
        (Err(copy_error), _) => Err(copy_error),
    }
}
```

(`RawSftpSession::open` — check the exact method name/signature in `rawsession.rs` around the `read`/`write` methods found at lines 316/343; adjust to the crate's actual open call. The `Handle` response's field carrying the handle string is what `fs::File::new` receives — see `src/client/fs/file.rs:41`. The typed error mapping: read errors become `CopyStage::ReadSource` inside the engine already; wrap-level errors here use `RemoteError::Transfer` with the path in the message, matching the sequential path's texts.)

Download additionally revalidates: call `revalidate_remote_fingerprint` is NOT needed because `fingerprint_open_remote` both stats and is the value handed to `on_fingerprint` in the same call — mirror the sequential `download_to_partial` order exactly: fingerprint remote, `on_fingerprint`, open raw read handle, open local `PositionalFile` sink, copy, close, `sync()` the local file before returning.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p termlab_remote 2>&1 | grep "test result"`
Expected: all green (new flag/fingerprint tests pass; raw-session code compiles — its live behavior is Task 8's).

- [ ] **Step 6: Commit**

```bash
git add crates/termlab_remote/src/transfer/sftp_io.rs
git commit -m "Add raw-session pipelined SFTP transfer entry points"
```

---

### Task 6: Runner seam, tuning flow, and sequential fallback

**Files:**
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/runner.rs` (trait `TransferIo` ~line 345, `RunnerServices` ~line 427, trait `SftpAttempt` ~line 414, the copy call ~line 668, `RealTransferIo`)
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/engine.rs` (the `.run(job, ...)` dispatch ~line 806)

**Interfaces:**
- Consumes: `PipelineTuning`, `upload_to_partial_pipelined`, `download_to_partial_pipelined` (Task 5); `QueueSettings.pipeline_depth`/`pipeline_chunk_bytes` (Task 2).
- Produces: `SftpAttempt::run(&self, job, control, reporter, tuning: PipelineTuning)`; `TransferIo::copy_ranges(...)` default method (sequential); `RealTransferIo::copy_ranges` override (pipelined when `tuning.depth > 1`, with first-window fallback). Engine converts settings → tuning at dispatch.

- [ ] **Step 1: Write the failing tests**

In `runner.rs`'s test module (using its existing fake `TransferIo`/job/reporter helpers — read two neighboring tests first and reuse their builders):

```rust
#[tokio::test]
async fn default_copy_ranges_preserves_sequential_behavior() {
    // Re-run an EXISTING upload happy-path test's setup verbatim, but through
    // a runner whose attempt receives PipelineTuning { depth: 16, chunk_bytes: 64 }.
    // The fakes do not override copy_ranges, so the default (sequential) path
    // must produce the identical outcome, checkpoints, and reporter events as
    // that existing test asserts. Copy those assertions here unchanged.
}

#[tokio::test]
async fn pipelined_failure_in_first_window_falls_back_to_sequential() {
    // Fake TransferIo that overrides copy_ranges: the first call (depth > 1)
    // returns Err(CopyError::Io { stage: CopyStage::WriteDestination,
    // kind: std::io::ErrorKind::Other, cause: "concurrency rejected".into() })
    // wrapped in the fallback marker; the runner must invoke copy_ranges a
    // second time with depth 1 and the attempt must complete successfully.
    // Assert: two copy_ranges invocations recorded (depths [16, 1]),
    // outcome Completed, one warn-level fallback log marker recorded.
}

#[tokio::test]
async fn pipelined_failure_after_first_window_is_a_real_error() {
    // Fake override reports the failure as NOT first-window (bytes advanced
    // beyond the initial window): runner must classify it exactly as the
    // sequential engine's WriteDestination failure (same terminal/retry
    // classification as the existing write-failure test asserts).
}
```

These are written as real tests, not comments: mirror the concrete setup of the nearest existing runner tests (`upload_...` happy path and the `..._write_failure_...` tests named around lines 5274/5286). The plan cannot paste those helpers verbatim here — copy their builder calls from the file when writing the tests, keeping the new assertions above.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p termlab_tauri runner 2>&1 | tail -5`
Expected: compile error — `copy_ranges` and the tuning parameter do not exist.

- [ ] **Step 3: Implement**

1. Add to `TransferIo` (default method, so every fake keeps sequential behavior):

```rust
    /// Copy `offset..total` from the opened source into the opened partial.
    /// The default is the sequential engine; the real IO overrides this to
    /// run the pipelined engine against its own positional handles.
    async fn copy_ranges(
        &self,
        _connection: &C,
        _job: &TransferJob,
        _artifacts: &ManagedArtifacts,
        source: &mut Self::Source,
        partial: &mut Self::Partial,
        offset: u64,
        total: u64,
        tuning: PipelineTuning,
        control: &mut (dyn FnMut() -> ControlDecision + Send),
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<CopyOutcome, CopyError> {
        copy_with_checkpoint_typed(
            source, partial, offset, total,
            tuning.chunk_bytes.max(1),
            control, progress,
        ).await
    }
```

Replace the direct `copy_with_checkpoint_typed` call at ~line 668 with `self.io.copy_ranges(connection, job, artifacts, &mut source, &mut partial, offset, total, tuning, &mut control_fn, &mut progress_fn)` — keeping every surrounding line (finalize, checkpoint persistence, outcome handling) untouched.

2. `RealTransferIo::copy_ranges` override — dispatch on direction, ignore the stream handles, call the Task 5 entry points with a no-op `on_fingerprint` that revalidates instead (the runner already established the fingerprint this attempt):

```rust
    async fn copy_ranges(&self, connection, job, artifacts, _source, _partial,
        offset, total, tuning, control, progress) -> Result<CopyOutcome, CopyError>
    {
        if tuning.depth <= 1 {
            // fall through to the sequential default by calling it explicitly
            return default_sequential_copy(...); // extract the default body into a free fn both share
        }
        let result = match job.direction {
            TransferDirection::Upload => upload_to_partial_pipelined(
                connection, &job.local_path, &artifacts.partial_remote_path(job), offset, tuning,
                |current| async move { verify_fingerprint_matches(job, current) }, control, progress).await,
            TransferDirection::Download => download_to_partial_pipelined(
                connection, &job.remote_path, &artifacts.partial_local_path(job), offset, tuning,
                |current| async move { verify_fingerprint_matches(job, current) }, control, progress).await,
        };
        match result {
            Err(error) if progress_never_advanced_past(offset, tuning) => {
                log::warn!("pipelined transfer degraded to sequential for job {}: {error}", job.id);
                Err(CopyError::pipeline_unsupported(error)) // marker the runner maps to one sequential retry
            }
            other => other.map_err(copy_error_from_remote),
        }
    }
```

The exact artifact-path accessors, fingerprint comparison, and error-marker plumbing must reuse what the runner already has: `ArtifactInventory`/`ManagedArtifacts` naming functions, the stored `job.source_fingerprint`, and the existing `TransferIoError`→classification mapping. Implement `pipeline_unsupported` as a new `CopyError` variant ONLY if a marker cannot ride the existing error text; prefer a small `enum CopyRangesError { Copy(CopyError), DegradeToSequential(CopyError) }` return type for `copy_ranges` if that keeps `CopyError` untouched — pick whichever compiles cleanest with the existing classification tests still green, and keep the "first window only" rule: track whether `progress` ever reported past `offset + depth × chunk_bytes`; if it did, never degrade.

3. Runner-side fallback: where the copy result is handled (~line 668's match), a `DegradeToSequential` result re-invokes `copy_ranges` once with `PipelineTuning { depth: 1, ..tuning }` and continues with that result.

4. `SftpAttempt::run` gains `tuning: PipelineTuning`; `RunnerServices` passes it through to the copy site. Engine dispatch (~engine.rs:806):

```rust
let tuning = PipelineTuning {
    depth: settings.pipeline_depth,
    chunk_bytes: settings.pipeline_chunk_bytes,
};
// existing: .run(job, RunnerControl::new(control_rx), reporter)
.run(job, RunnerControl::new(control_rx), reporter, tuning)
```

where `settings` is the engine state's current `QueueSettings` snapshot at dispatch time (the engine already reads settings for scheduling; reuse that access). Update every test double implementing `SftpAttempt` (compiler lists them) to accept and record the tuning.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p termlab_tauri 2>&1 | grep "test result"`
Expected: all green — the three new tests plus every existing runner/engine/scheduler test unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/src/remote/transfer_queue/runner.rs crates/termlab_tauri/src/remote/transfer_queue/engine.rs
git commit -m "Route transfer attempts through the pipelined copy seam"
```

---

### Task 7: Concurrency dialog exposes pipeline tuning

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/features/transfers/dialogs.js` (`showConcurrency`, ~line 255)
- Test: `scripts/tests/test_transfer_center.mjs` (the existing concurrency-dialog block around line 593)

**Interfaces:**
- Consumes: `settings.pipelineDepth` / `settings.pipelineChunkBytes` from the snapshot (Task 2's TS shape); `updateSettings` passes the object through unchanged.
- Produces: dialog fields `data-transfer-field="pipeline-depth"` (1–64) and `data-transfer-field="pipeline-chunk-kib"` (32–1024, KiB in the UI, bytes on the wire).

- [ ] **Step 1: Extend the failing test**

In the existing concurrency test block (which sets `global-limit`/`per-host-limit` and asserts the `updateSettings` runtime call), add before the Save click:

```javascript
  concurrency.bodyEl.querySelector('[data-transfer-field="pipeline-depth"]').value = '8';
  concurrency.bodyEl.querySelector('[data-transfer-field="pipeline-chunk-kib"]').value = '512';
```

and extend the settings assertion to:

```javascript
  assert.deepStrictEqual(harness.runtimeCalls.at(-1), {
    method: 'updateSettings',
    args: [{ globalLimit: 6, perHostLimit: 3, pipelineDepth: 8, pipelineChunkBytes: 512 * 1024 }],
  });
```

Also add a validation case: set `pipeline-depth` to `'0'`, click Save, assert the dialog's error element is visible and no `updateSettings` call was recorded.

(Adjust the exact assertion shape to match how that block currently asserts the call — read it first; keep its style.)

- [ ] **Step 2: Run to verify failure**

Run: `node scripts/tests/test_transfer_center.mjs`
Expected: fails — `pipeline-depth` field not found.

- [ ] **Step 3: Implement**

In `showConcurrency`, generalize `limitField`'s min/max (add parameters) and add after the two existing fields:

```javascript
        depthInput = limitField(bodyEl, 'Pipeline depth', 'pipeline-depth',
          current.pipelineDepth || 16, 1, 64);
        chunkInput = limitField(bodyEl, 'Chunk size (KiB)', 'pipeline-chunk-kib',
          Math.round((current.pipelineChunkBytes || 262144) / 1024), 32, 1024);
```

Extend the Save handler: validate both with the same integer helper (parameterized bounds), keep one shared error element with updated copy ('Enter whole numbers within each field's range.'), and submit:

```javascript
            return closeAfter(handleRef, onSave, {
              globalLimit, perHostLimit,
              pipelineDepth: depth,
              pipelineChunkBytes: chunkKib * 1024,
            });
```

- [ ] **Step 4: Run to verify pass**

Run: `node scripts/tests/test_transfer_center.mjs`
Expected: all assertions passed. Then the full frontend sweep: `for f in scripts/tests/test_*.mjs; do node "$f" >/dev/null || echo "FAIL: $f"; done` — no failures.

- [ ] **Step 5: Commit**

```bash
git add crates/termlab_tauri/frontend/app/features/transfers/dialogs.js scripts/tests/test_transfer_center.mjs
git commit -m "Expose pipeline depth and chunk size in the concurrency dialog"
```

---

### Task 8: Live verification, docs, and the full gate

**Files:**
- Modify: the live-gated test module (locate with `grep -rn "resolve_live_sftp_connection" crates/termlab_tauri/src` — the same env-gated harness the queue project used; follow its skip-when-unconfigured pattern exactly)
- Modify: `docs/superpowers/specs/2026-08-25-sftp-pipelined-transfers-design.md` (status)
- Modify: `docs/superpowers/notes/sftp-transfer-queue-manual-checklist.md` (evidence rows)

**Interfaces:**
- Consumes: everything prior.
- Produces: recorded before/after throughput evidence and a green workspace.

- [ ] **Step 1: Add the live pipelined cases**

Following the existing live harness's connection/env pattern (same gating, same skip messaging), add:

1. `live_pipelined_upload_matches_content_and_beats_sequential`: generate a 64 MiB random temp file; upload once with `PipelineTuning { depth: 1, chunk_bytes: 262144 }` and once with `{ depth: 16, ... }` to distinct remote paths, timing each; assert both remote files' sizes match and (fetch back or remote-hash if the harness has an exec helper) contents match the source; assert pipelined elapsed < sequential elapsed, and log both throughputs so the numbers land in test output.
2. `live_pipelined_download_resumes_after_interruption`: start a pipelined download with a control callback that returns `Pause` after ~25% progress; assert the outcome is `Paused` with a frontier < total; re-invoke with `offset = frontier` and assert completion plus a byte-for-byte match against the remote source.

(Latency injection is environment-dependent; the throughput assertion is `<`, not a ratio, so it holds on LAN too. Record the measured numbers in the checklist row.)

- [ ] **Step 2: Run what the environment allows**

Run: `cargo test -p termlab_tauri --release live_pipelined 2>&1 | tail -5` with the harness env configured if a disposable server is available; otherwise confirm the tests skip cleanly and mark the checklist rows as pending-live-evidence (the same convention the queue project used).

- [ ] **Step 3: Reconcile the docs**

- Spec status: `**Status:** Approved` → `**Status:** Implemented` plus a short implementation note naming the branch and the commit range, mirroring the queue spec's convention.
- Manual checklist: add a "Pipelined transfers" section with two evidence rows (throughput before/after; pause→resume content hash) and whatever numbers Step 2 produced.

- [ ] **Step 4: Full gate**

Run, and require all green (modulo the two documented pre-existing exceptions: workspace `cargo fmt` drift and `tl-dialog.js:334` in the boundary check):

```bash
cargo test --workspace 2>&1 | grep -E "test result|FAILED"
for f in scripts/tests/test_*.mjs; do node "$f" >/dev/null || echo "FAIL: $f"; done
bash scripts/check_frontend_boundaries.sh
```

- [ ] **Step 5: Commit and push**

```bash
git add -A docs crates
git commit -m "Verify pipelined transfers and reconcile docs"
git push
```

---

## Self-Review

- **Spec coverage:** frontier contract → Task 1; settings/additive schema → Task 2; bounded-memory window + depth-1 equivalence + typed provenance + pause drain → Task 4; raw session on the existing connection, open-flag parity, limits clamp, reopen revalidation → Task 5; engine substitution, first-window fallback, unchanged runner flow → Task 6; settings UI → Task 7; live throughput/resume evidence + docs status → Task 8. Progress-frontier reporting is asserted in Tasks 4 (engine) and inherited unchanged by the runner (Task 6 parity test).
- **Placeholder check:** Task 6 Step 1 deliberately instructs copying the existing runner-test builders rather than pasting them (they are hundreds of lines and live in the same file the implementer edits); each new test's distinct assertions are spelled out. Task 5 flags two russh-sftp API details (`open` signature, limits negotiation) with exact file/line pointers and a defined fallback — resolved at `cargo check` time, not left open.
- **Type consistency:** `PipelineTuning { depth, chunk_bytes }` is defined once (Task 4) and consumed by Tasks 5–6; settings names `pipeline_depth`/`pipeline_chunk_bytes` (Rust) ↔ `pipelineDepth`/`pipelineChunkBytes` (wire/TS/dialog) are consistent across Tasks 2, 6, 7.
