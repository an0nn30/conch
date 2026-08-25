# Pipelined SFTP Transfers — Design

**Status:** Implemented
**Date:** 2026-08-25
**Scope:** Make single-file SFTP uploads and downloads approach link capacity on
latent connections by keeping a bounded window of chunk requests in flight,
without changing the durable transfer queue's persistence, recovery, or UI
contracts. This is the first sub-project of the "fast transfers" track; bulk /
small-file throughput and delta re-transfers are separate follow-on projects.

**Implementation:** branch `feat/sftp-pipelined-transfers`; commits `044febc`
through `e87c458`, plus the Task 8 verification commit that reconciled this
status.
Manual and live-evidence tracking is in the
[SFTP transfer queue manual checklist](../notes/sftp-transfer-queue-manual-checklist.md)'s
"Pipelined transfers" section.

Automated frontier, scheduler, engine-substitution, runner-parity, and
settings-UI coverage was green before this status changed. The two live
throughput/resume tests added in Task 8
(`live_pipelined_upload_matches_content_and_beats_sequential`,
`live_pipelined_download_resumes_after_interruption`) were invoked but skipped
before connecting because disposable-server credentials were not present in
this environment; the checklist rows for their evidence remain
pending-live-evidence. The repository-wide formatter and frontend boundary
checks retain their verified clean-base exceptions: unrelated Rust formatting
drift and `frontend/app/ui/tl-dialog.js:334`, respectively.

## Problem

The current copy engine (`termlab_remote/src/transfer/copy.rs`) is strictly
sequential: read one chunk (≤ 256 KiB), await it, write it, await it, repeat.
Every chunk costs at least one full round trip in each direction, so throughput
is capped at roughly `chunk_size / RTT` regardless of link bandwidth. On the
LAN this is tolerable; over Tailscale/WAN links it leaves most of the link idle.

The SFTP protocol does not impose this: read and write requests carry explicit
offsets and ids, and servers process many outstanding requests on one open
handle. `russh-sftp`'s `RawSftpSession` exposes exactly this — per-request
`read(handle, offset, len)` / `write(handle, offset, data)` multiplexed by id —
and auto-negotiates the server's `limits@openssh.com` read/write caps.

## Product rules

1. **Same contracts, faster bytes.** Queue state, checkpoints, fingerprints,
   managed partials, conflict handling, the commit protocol, events, and the
   Transfer Center are unchanged. Pipelining is an engine substitution inside a
   transfer attempt, invisible except for speed.
2. **The durable checkpoint is always a contiguous frontier.** Nothing beyond
   the highest all-bytes-complete offset is ever persisted as progress. Bytes
   written out of order past the frontier are disposable and are reconciled by
   the existing truncate-to-checkpoint logic on resume.
3. **Bounded memory.** In-flight data is capped at `window_depth × chunk_size`
   (default 16 × 256 KiB = 4 MiB per active transfer attempt).
4. **Graceful degradation.** Any indication a server cannot handle concurrent
   requests degrades that attempt to the sequential engine with a log line —
   never a failed job for pipelining's own sake.
5. **No new connections or credentials.** The engine opens one additional
   channel on the already-authenticated SSH connection for its raw SFTP
   session. No re-authentication, no second TCP connection.

## Architecture

### Placement

- **`termlab_remote/src/transfer/pipelined.rs`** — the engine. Owns the window
  scheduler, contiguous-frontier tracker, and typed outcome mapping. Exposes
  the same outcome shape as `copy_with_checkpoint` so the runner treats both
  engines uniformly.
- **`termlab_remote/src/transfer/frontier.rs`** — pure contiguous-frontier
  bookkeeping (completed-range set → highest contiguous offset), unit-testable
  without I/O.
- **`termlab_remote/src/transfer/sftp_io.rs`** — gains raw-session plumbing:
  open a `RawSftpSession` on a new channel of the existing connection, open a
  file handle with the same flags the current path uses (resume preserves
  content, fresh transfer truncates), fstat, setstat-truncate, close.
- **Queue runner (`transfer_queue/runner.rs`)** — selects the pipelined engine
  for SFTP↔local file copies; everything else (checking, fingerprints,
  conflicts, commit, retries) is untouched. Selection is per attempt, so a
  degraded attempt can fall back without affecting the job's state machine.

### The window scheduler

For a transfer of `total` bytes starting at durable offset `start`:

1. Compute `chunk_size = min(configured, server_limit, 256 KiB)`.
2. Issue up to `window_depth` requests for consecutive chunk offsets.
   - **Download:** remote `read(handle, offset, len)` → local positional write.
   - **Upload:** local positional read → remote `write(handle, offset, data)`.
3. As each completion arrives: record the chunk in the frontier tracker, issue
   the next pending chunk (keeping the window full), and check control signals.
4. A short read below the requested length at EOF closes the tail exactly as
   the sequential engine does; a short read elsewhere is a typed read error.
5. When all chunks complete, flush and close both sides, then return the same
   outcome the sequential path returns.

Completions arriving out of order only advance displayed/persisted progress
when they extend the contiguous frontier. The engine never reports or persists
progress containing holes.

### Checkpoints, pause, cancel, errors

- **Persisted checkpoint:** the frontier, coalesced on the same cadence the
  queue already uses for progress persistence. Restore-time reconciliation is
  the existing logic: truncate the managed partial to the durable checkpoint,
  which also disposes of any out-of-order bytes beyond the frontier.
- **Pause:** stop issuing, drain in-flight completions, flush and close,
  persist the frontier, return the paused outcome. The drain is bounded by the
  window (at most `window_depth` outstanding awaits).
- **Cancel:** identical drain, then the runner's existing artifact cleanup.
- **Chunk error:** abort the window (remaining completions are awaited and
  discarded), map to the same typed stage provenance (`ReadSource`,
  `WriteDestination`, seek stages) the sequential engine reports, so the
  queue's retry classification is unchanged.
- **Degradation triggers:** the raw session rejecting concurrent requests,
  limit-related failures on the first window, or protocol-version handshakes
  without the needed guarantees → retry the attempt once with the sequential
  engine, log the reason. Persist nothing about the degradation; it is
  re-evaluated per attempt.

### Configuration

Two new fields on the existing queue settings (persisted in the transfer
store's settings block, additive and defaulted for older stores):

- `pipelineDepth` — default 16, range 1–64. Depth 1 must behave observably
  like the sequential engine.
- `pipelineChunkBytes` — default 262144 (256 KiB), clamped to the server's
  negotiated limit at attempt time.

The Concurrency dialog gains these two fields with the same validation
plumbing as the global/per-host limits. Bandwidth throttling remains out of
scope, but the scheduler's issue-loop is the single place a rate limiter would
later hook.

### Progress and events

`bytesTransferred` = frontier. Speed and ETA derive from it as today. No new
events, no payload changes, no frontend changes outside the settings dialog.

## Failure behavior

- Mid-window disconnect: drain fails fast, typed transport error, existing
  persisted-retry path; resume reconciles to the frontier checkpoint.
- Crash mid-window: restart restores `Paused` per the queue design; the
  partial may contain bytes beyond the checkpoint, which reconcile truncates.
- Disk full on positional local write: same typed failure and
  preserve-partial behavior as the sequential engine.
- Server without `limits@openssh.com`: chunk size falls back to 256 KiB and
  OpenSSH-conventional caps; correctness never depends on the extension.

## Testing strategy

### Rust unit tests

- `frontier.rs`: out-of-order insertion, duplicate completion, gap handling,
  frontier advance exactness — pure, exhaustive.
- Scheduler against a deterministic fake offset store: window stays full,
  completions reordered arbitrarily, issue order recorded; pause drains and
  reports the frontier; cancel drains; a failing chunk aborts and maps typed
  provenance; depth 1 equals sequential behavior byte-for-byte.
- Settings: serde defaults for the two new fields against a v1 store without
  them; clamping.
- Degradation: fake session that rejects concurrency → attempt completes via
  sequential fallback with one log marker.

### Integration tests

- Runner-level: pipelined upload and download against the existing fake
  endpoints, pause → resume at frontier, crash-style interrupt → truncate
  reconcile, retry classification parity with sequential.

### Live verification (opt-in harness)

- Big-file upload and download against OpenSSH with an artificial-latency
  path where available; assert pipelined throughput exceeds sequential on the
  same link; mid-transfer kill then explicit resume completes with matching
  content hash. Record before/after numbers in the manual evidence notes.

## Non-goals

- Segmented multi-channel transfers (the frontier model is deliberately
  range-capable so per-range frontiers can extend it later, but v1 is one
  channel, one window).
- Delta / rsync-style re-transfers.
- FTP, FTPS, SCP, or any new protocol.
- Bandwidth throttling and scheduling changes.
- Directory/bulk transfer changes — the engine accelerates each file; how
  many files run at once remains the queue scheduler's existing policy.

## Success criteria

1. A single large file over a latent link transfers materially faster than
   the sequential engine (target: ≥ 4× on a ≥ 20 ms RTT path; measured and
   recorded).
2. Pause, resume, cancel, retry, conflict, and commit behavior are
   observably identical to the sequential engine in every automated test.
3. A crash or disconnect mid-window never yields a partial whose persisted
   checkpoint overstates contiguous progress.
4. Depth 1 reproduces sequential behavior, and degradation falls back
   automatically without failing the job.
5. Older persisted stores load with defaulted settings; no schema version
   bump is needed.
