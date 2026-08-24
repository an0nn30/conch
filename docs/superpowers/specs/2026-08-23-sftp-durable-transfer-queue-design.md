# SFTP Durable Transfer Queue and Transfer Center — Design

**Status:** Draft for implementation-plan review
**Date:** 2026-08-23
**Scope:** Replace the current fire-and-forget SFTP transfer registry with a Rust-owned, durable, resumable queue and expose it through a desktop-native Transfer Center. The design is SFTP-first, while keeping persisted job records extensible enough for later FTP/FTPS/SCP adapters.

## Product rules settled in brainstorming

1. **The backend owns transfer truth.** Queue state, scheduling, checkpoints, retries, conflicts, and history live in Rust. Every frontend window projects the same backend snapshot and event stream.
2. **The latest source is opened for each new transfer.** A new upload or download reopens and restats its source when the job reaches `Checking`, immediately before the first byte is read. It must never reuse a file handle, SFTP handle, directory-listing object, or source metadata captured when the app or panel opened.
3. **Jobs survive restart.** Queued, paused, attention-required, and terminal jobs persist across app restarts. A job that was running when the app stopped restores as paused. Restoring state never initiates network activity.
4. **Resume is byte-level and validated.** Managed partial files and durable byte checkpoints support real resume. Once a transfer attempt has established its source fingerprint, source size or modification-time changes require an explicit restart; the application never silently combines bytes from different source versions.
5. **Conflicts do not block the whole queue.** An affected job enters `Needs attention`. The user can Resume when a compatible managed partial exists, Overwrite, Rename, or Skip. Unrelated jobs continue.
6. **Scheduling is bounded and configurable.** Defaults are three active transfers globally and two per host. Jobs targeting the same normalized destination are always serialized, regardless of the configured limits.
7. **Ad-hoc endpoints may persist; credentials may not.** A job can persist a non-secret host, port, user, and proxy/jump-host snapshot. Passwords, key passphrases, private-key contents, vault secrets, and live session handles are never written to the transfer store.
8. **No background authentication.** A restored job without a usable live connection enters `Needs connection`. The user explicitly invokes the existing connect/authentication flow and then resumes the job.
9. **The existing transfer commands remain compatible.** Existing upload/download entry points enqueue and return a job UUID. Existing progress listeners continue receiving compatible progress and terminal notifications during migration.
10. **The first UI is a dense bottom Transfer Center.** It uses the existing tool-window system, so it can be docked, resized, hidden, or popped out. Lightweight toasts summarize batches; detailed control stays in the Transfer Center.

## Context and gap reconciliation

`SFTP_FILEZILLA_GAP.md` is useful as a product backlog, but several entries marked missing are already present in the repository: connection CRUD and folders, SSH-config import/export, independent SFTP host connections, proxy/jump-host fields, local/remote navigation, hidden-file controls, transfer progress/cancellation, and remote editor integration.

The remaining high-value work clusters are:

1. Durable queue, pause/resume, bounded concurrency, retries, conflicts, and transfer history.
2. Recursive/bulk transfer and drag-and-drop.
3. Directory comparison and synchronization.
4. Advanced browsing and file operations such as bookmarks, search, permissions, and symlink handling.
5. Connection resilience and security improvements.
6. FTP, FTPS, and SCP transport support.

This design covers cluster 1 for SFTP. The other clusters remain separate projects that can build on the queue.

The current implementation starts transfer tasks directly from `termlab_remote` and keeps only active cancellation handles in memory. The Tauri command layer returns a UUID and forwards progress, but there is no durable job model, scheduler, or restart recovery. The existing SFTP freshness fix that explicitly flushes and closes destination handles remains a prerequisite; the queue design adds a stronger rule that source handles and fingerprints are acquired per job execution rather than cached across operations.

## Architecture

### Ownership boundaries

The design uses two Rust layers and a thin frontend projection:

- **`termlab_remote`: SFTP transfer primitives.** Opens fresh local and remote handles, stats sources, seeks to offsets, copies chunks, flushes/closes handles, truncates managed partials when reconciling checkpoints, and performs SFTP rename/delete operations. This layer reports typed outcomes but does not own persistent queue policy.
- **Tauri remote/transfer queue module: orchestration.** Owns the job reducer, atomic store, scheduler, concurrency and destination locks, retry clock, connection binding, conflict decisions, recovery, and event emission.
- **Frontend transfer feature: projection and controls.** Loads one initial snapshot, applies backend job/summary events, and invokes typed queue commands. It does not independently infer job state or schedule work.

The persisted model includes a versioned protocol discriminator and endpoint records from the beginning. Version 1 dispatches directly to the SFTP runner. A formal transport trait is deferred until a second protocol exists and supplies concrete evidence for the shared interface; the initial design must not force SFTP through a speculative abstraction.

### Job identity and persisted record

Every job has a stable UUID used by commands, events, managed artifact names, and history. The versioned persisted record contains:

- schema version and job UUID;
- protocol (`sftp` in version 1) and direction (`upload` or `download`);
- origin (`files_panel`, `editor`, or another named producer);
- connection reference: configured server entry ID or an ad-hoc non-secret endpoint snapshot;
- local and remote source/destination paths;
- normalized destination-lock key;
- user-visible name and optional batch ID;
- priority class plus stable user ordering;
- state and attention/error detail;
- source fingerprint, once established;
- durable checkpoint and expected total bytes;
- retry attempt, maximum attempts, and next retry time;
- conflict resolution selected for this attempt;
- managed partial and backup artifact names;
- commit/recovery phase;
- created, updated, started, and finished timestamps.

Runtime-only fields—live SSH/SFTP sessions, cancellation tokens, open handles, instantaneous speed samples, and pending task handles—are never serialized.

Configured endpoints persist a stable server entry ID. Ad-hoc endpoints persist only host, port, user, proxy command, and jump-host configuration needed to identify the destination and guide an explicit reconnect. Credentials remain in the existing vault or in memory for a live authenticated session.

### Source freshness and fingerprint rules

When a new job reaches `Checking`, the runner:

1. Opens the source by its current path.
2. Reads size and modification time from the live filesystem or SFTP server.
3. Stores the fingerprint immediately before transferring the first byte.
4. Emits and persists the transition before entering `Running`.

This guarantees that repeated uploads/downloads see the latest on-disk version, even if the panel was opened before the file changed.

Once a source fingerprint is established for an attempt, retry and resume must match both size and modification time. A mismatch moves the job to `Needs attention` with Restart and Skip actions. Restart deletes the managed partial, clears the checkpoint and old fingerprint, and returns the job to `Queued`, where it captures the current source version during a new `Checking` pass.

If a server cannot provide a modification time, the job can complete a fresh transfer but cannot claim safe byte-level resume after interruption. Such a job offers Restart rather than Resume. The implementation must not treat equal size alone as proof that two source versions are identical.

### State machine

The normal flow is:

```text
Queued -> Connecting -> Checking -> Running -> Completed
```

Supported side paths are:

```text
Running <-> Paused
Connecting/Checking/Running -> Needs connection
Checking/Running -> Needs attention
Connecting/Checking/Running -> Retry waiting -> Queued
Any non-terminal state -> Cancelled
Non-retryable or exhausted error -> Failed
```

`Needs attention` carries a typed reason such as destination conflict, source changed, commit recovery ambiguity, or cleanup failure. `Needs connection` is distinct from failure because no transfer attempt is made until the user reconnects. Terminal states are `Completed`, `Failed`, and `Cancelled`.

All state changes go through a pure transition function. Illegal transitions are rejected and logged rather than repaired with frontend flags. The backend persists a transition before publishing the corresponding event, so a window can always recover by requesting a fresh snapshot.

### Durable store and restart recovery

Queue state is stored as versioned JSON under the TermLab application config directory, conventionally `~/.config/termlab/transfers.json` on platforms using that layout. The actual path must come from the same platform-aware application-config resolver used elsewhere in the app.

Writes use the repository's atomic-file pattern: serialize to a sibling temporary file, flush it, and rename it over the store. File permissions should be user-only where the platform supports them. Immediate writes occur for state transitions, conflict decisions, connection references, retry scheduling, checkpoint barriers, and commit phases. Display-only progress is coalesced so sustained transfers do not rewrite the store for every chunk.

The durable checkpoint may trail displayed progress by one persistence interval. On restore, the runner compares the managed partial's actual size with the persisted checkpoint, uses only the last durable checkpoint, and truncates uncommitted trailing bytes before resuming. If safe reconciliation is unavailable, the job enters `Needs attention` rather than guessing.

Restart behavior is deterministic:

- `Running`, `Connecting`, and `Checking` restore as `Paused`.
- `Queued`, `Paused`, `Needs connection`, `Needs attention`, and `Retry waiting` remain represented without starting work.
- No restored job reconnects, retries, or transfers until the user resumes the queue or the individual job.
- Missing managed partials, changed sources, and ambiguous commit artifacts become typed attention states.
- Active jobs are never evicted. Terminal history is capped at the newest 500 jobs after compaction.

If the JSON store cannot be parsed or migrated, it is renamed to a timestamped quarantine file. The app starts with an empty inactive queue and surfaces a persistent recovery error that names the quarantine location. It must never silently overwrite the only corrupted copy or initiate network work after a failed restore.

### Scheduler

The scheduler is a single backend coordinator shared by all windows. It selects runnable jobs by:

1. priority class (`interactive` editor work before normal user transfers);
2. explicit user order within a class;
3. creation time as the stable tie-breaker.

Jobs that need input, connection, or a future retry time are skipped without blocking later runnable work. The default limits are three active jobs globally and two per host, both configurable. Connecting, checking, and running attempts consume a slot so slow connection setup cannot bypass the limits.

A normalized destination key is locked for the full attempt, including commit. Upload keys identify the remote endpoint and normalized remote destination path. Download keys identify the normalized absolute local destination path. Two jobs with the same destination never execute concurrently even if they use different windows or were enqueued by different producers.

Pause All stops dispatching new work and cooperatively pauses active jobs. Resume All makes eligible paused/queued jobs runnable; jobs requiring connection or attention remain untouched. Reordering changes only eligible queue order and never interrupts an active commit.

### Pause, resume, cancel, and retry

Transfers copy bounded chunks and check control signals between chunks.

- **Pause:** finish the current chunk, flush and close the destination handle, persist the durable checkpoint, release scheduler locks, then enter `Paused`. The managed partial remains.
- **Resume:** reacquire a connection and locks, revalidate the source fingerprint, reconcile the managed partial against the durable checkpoint, reopen both sides at that offset, and continue.
- **Cancel:** stop at a chunk boundary, close handles, delete artifacts managed exclusively by that job, persist `Cancelled`, and release locks. A cleanup failure is recorded explicitly so a leftover artifact is discoverable.
- **Retry:** transient transport failures use at most three automatic attempts with exponential backoff. Retry scheduling is persisted. Authentication errors, permission errors, conflicts, source changes, and commit ambiguities never retry automatically.

The first attempt captures a source fingerprint. A later retry does not silently adopt a modified source; the user must choose Restart to begin a new attempt against the latest version.

### Managed partials and destination conflicts

The runner never streams directly into the final destination. It creates a unique partial in the destination directory, scoped to the job UUID. Keeping it in the destination directory permits same-filesystem rename on local downloads and the closest SFTP equivalent on uploads.

An arbitrary pre-existing destination is never treated as a resumable partial and is never appended to. The Resume action is enabled only when a job-owned partial, checkpoint, and compatible source fingerprint exist. Otherwise a conflict offers:

- **Overwrite:** preserve the current final as a temporary job-owned backup during commit;
- **Rename:** choose a new final destination, recalculate its destination-lock key, and recheck conflicts;
- **Skip:** mark the job completed-with-skip in its result metadata without changing the destination;
- **Resume:** continue only the compatible managed partial.

Conflict resolution affects only the selected job or an explicitly selected batch. Dialogs must not stall the scheduler thread or block unrelated jobs.

### Recoverable commit protocol

The SFTP library exposes standard rename rather than a guaranteed POSIX atomic overwrite. Overwrite therefore uses a recoverable swap for both local and remote destinations:

1. Persist a commit phase indicating preparation.
2. If a final exists, rename it to a unique job-owned backup and persist that phase.
3. Rename the completed managed partial to the final name and persist promotion.
4. Delete the backup and persist completion.

If promotion fails after the backup move, the runner attempts to restore the backup. Every destructive step is preceded or followed by a durable phase marker sufficient for startup recovery. Recovery prefers preserving an intact old or new file over automatic cleanup. If artifact combinations are ambiguous, the job enters `Needs attention`; it never deletes the only intact copy.

When no final exists, commit can rename the partial directly, but it still records enough phase information to distinguish an unfinished transfer from a completed promotion after a crash.

### Connection binding and authentication

Jobs refer to a connection identity, not a frontend pane or serialized live session. At dispatch time the coordinator may bind a currently authenticated SFTP session that matches the configured server ID or ad-hoc endpoint identity. If none exists, the job enters `Needs connection`.

The scheduler never opens an authentication dialog itself. Selecting Connect from the job or Transfer Center invokes the existing explicit host connection and vault/password flow. On success, the backend associates the live connection and the user resumes the job. After application restart, jobs always require this explicit step before network activity.

### Tauri commands and events

The existing upload/download commands remain as compatibility entry points. They create a queue job, return its UUID immediately, and emit the existing progress shape while the job runs. Editor-origin jobs use interactive priority and keep their current save/open feedback by awaiting terminal job status rather than bypassing the queue.

New command responsibilities are:

- obtain the authoritative queue snapshot and summary;
- pause/resume one job or all eligible jobs;
- cancel or retry a job;
- resolve a conflict or source-change decision;
- set priority and reorder queued work;
- connect/rebind a job through the existing authentication flow;
- clear eligible terminal history;
- read/update concurrency settings.

Commands use typed serializable request/response structures. Frontend code accesses them through one transfer service module rather than scattering raw Tauri `invoke` calls through row components.

The backend emits:

- `transfer-job-updated` for a complete updated job projection or removal marker;
- `transfer-queue-summary` for counts, aggregate activity, and Pause All state;
- the existing transfer progress/terminal events during the compatibility period.

Events may be coalesced for byte progress but not for state transitions. A subscriber that detects a revision gap requests a fresh snapshot rather than reconstructing missing transitions. The queue is global, so events and snapshots have identical meaning in docked, popped-out, and multiple main windows.

## Transfer Center UX

The Transfer Center is a registered tool window using the existing docking and pop-out infrastructure. Its default presentation is a dense bottom table with these columns:

- file and direction;
- host;
- destination;
- status and progress;
- speed and ETA;
- row actions.

The toolbar provides Active/History views, Pause All/Resume All, clear completed history, and queue counts. Failures remain visible with Retry and error details. Attention rows provide an inline Resolve action that opens a focused dialog for Resume, Overwrite, Rename, or Skip, enabling only actions valid for that job.

Keyboard behavior is desktop-first:

- arrow keys move the selected row;
- Space pauses or resumes an eligible job;
- Enter opens details or resolution;
- Delete requests confirmation before cancel;
- a configurable application shortcut focuses the Transfer Center.

Rows and custom controls expose semantic labels, visible focus, and status text that does not rely on color alone. High-frequency progress updates should patch only affected rows and aggregate counters, not rebuild the full tool-window tree.

The frontend maintains only ephemeral presentation state such as the selected row, Active/History tab, and dialog visibility. Job status, progress, errors, retry time, and available domain actions come from the backend projection. A batch of jobs produces lightweight summary toasts instead of one toast per file.

## Failure behavior

- Source missing or changed: `Needs attention` with Restart/Skip.
- Destination conflict: `Needs attention` with the valid conflict actions.
- No authenticated session: `Needs connection`, with no automatic prompt.
- Transient disconnect/time-out: persisted exponential retry, then `Failed` after exhaustion.
- Authentication or permission failure: no automatic retry.
- Disk full: close handles, preserve the managed partial and checkpoint, and fail with actionable detail.
- Managed partial missing or incompatible: `Needs attention`; never append to an arbitrary destination.
- Commit failure: recover or restore from the persisted phase; ambiguous cases require attention.
- Corrupt queue store: quarantine and report; start no work.

Errors and logs may contain job IDs, endpoint labels, paths, states, and transport diagnostics, but never credentials or secret material.

## Implementation sequence

### Phase 1: Queue foundation

Add the versioned job model, pure state reducer, atomic persistence, authoritative snapshots/events, and compatibility wrappers so existing upload/download commands enqueue jobs.

### Phase 2: Resumable SFTP runner

Add fresh source open/stat behavior, fingerprints, managed partials, checkpoint reconciliation, pause/resume, retries, concurrency limits, destination locks, and recoverable commit.

### Phase 3: Transfer Center UI

Register the tool window and add Active/History views, row actions, resolution dialogs, keyboard behavior, concurrency settings, editor integration, and multi-window synchronization.

### Phase 4: Recovery and hardening

Cover restart recovery, corrupt-store quarantine, commit rollback, disk-full/permission failures, deterministic scheduler stress tests, and live OpenSSH integration tests.

### Phase 5: Documentation and migration cleanup

Document the queue schema and operational behavior, remove the obsolete active-only registry only after compatibility is proven, and reconcile the gap document with the implemented product.

Each phase must leave the repository testable. Production rollout occurs only after the queue, SFTP runner, and minimum Transfer Center controls work together; intermediate backend phases may remain behind an internal feature/migration boundary if exposing them would regress the current UI.

## Testing strategy

### Rust unit tests

- exhaustive legal and illegal state transitions;
- source fingerprint capture and mismatch decisions;
- schema round-trip, migration, atomic replacement, and corrupt-file quarantine;
- restore mapping of in-flight states to paused without dispatch;
- priority/order selection, global and per-host limits, and destination serialization;
- Pause All and explicit-resume behavior;
- retry classification, persisted timing, attempt exhaustion, and deterministic backoff;
- partial/checkpoint reconciliation and truncation;
- conflict action validation;
- recoverable commit phases, rollback, and ambiguous-artifact handling;
- history cap that never evicts active work;
- credential-free serialization.

The scheduler tests use a deterministic fake runner and fake clock. They must not depend on wall-clock sleeps or real network timing.

### Rust integration tests

- compatibility commands return a job ID and produce expected legacy and new events;
- initial snapshot plus incremental events converge to the authoritative queue;
- multiple frontend window labels observe and control one global queue;
- editor-origin work receives interactive priority without bypassing locks;
- pause/resume reopens sources and destinations at the durable offset;
- a second upload/download created after the source changes reads the new on-disk content;
- a paused job whose source changes refuses resume until Restart;
- overwrite commit restores the previous final when promotion fails.

A small live OpenSSH/SFTP fixture validates seek/resume, remote stat behavior, managed remote partials, rename/backup recovery, and disconnect handling. Most failure permutations remain under deterministic fakes so the suite is fast and reliable.

### Frontend tests

- snapshot hydration and revision-gap refresh;
- Active/History filtering and stable row selection under updates;
- row status, progress, speed/ETA, and valid actions;
- Pause/Resume, Cancel confirmation, Retry, clear completed, and Pause All;
- conflict/source-change dialogs with invalid actions disabled;
- keyboard navigation and focus restoration;
- docked and popped-out mounting through the same registration;
- two windows reflecting the same backend update without mirrored domain state;
- bounded row updates under rapid progress events;
- batch-summary toast behavior and accessible labels.

### Manual verification

- modify a local file after opening the app, upload it twice, and verify the second remote result is the latest local bytes;
- modify a remote file after opening the app, download it twice, and verify the second local result is the latest remote bytes;
- pause, quit, relaunch, reconnect explicitly, and resume both directions;
- change a source while paused and verify Resume is rejected until Restart;
- exercise Overwrite/Rename/Skip and verify unrelated jobs continue;
- stress global/per-host limits across two windows and repeated destinations;
- interrupt each recoverable commit phase and verify no old/new final is silently lost;
- inspect the persisted JSON and logs for credential leakage;
- judge bottom-panel density, resizing, keyboard flow, error clarity, and progress rendering under load.

## Non-goals and do-not-change-yet areas

- Recursive directory expansion, multi-select batches, and drag-and-drop are the next transfer project, not hidden inside the version 1 queue implementation.
- Directory comparison, mirroring, and synchronization wait until recursive job expansion and conflict semantics are stable.
- Bookmarks, remote search/filtering, chmod/chown, symlink policy, previews, and advanced file operations remain separate browsing work.
- Keepalive, automatic reconnect, agent forwarding, and broader credential/security changes remain separate connection work. This queue explicitly avoids auto-connect.
- FTP, FTPS, and SCP are not implemented here. The persisted protocol discriminator is forward-compatible, but no speculative transport trait is introduced until the second protocol is designed.
- Bandwidth limiting is deferred. The scheduler should permit a later rate limiter without coupling it to frontend state, but version 1 does not promise throttling.
- The existing server editor, host folders, vault, independent SFTP session flow, file panes, and editor surface are not redesigned by this project.
- `SFTP_FILEZILLA_GAP.md` should not drive duplicate implementation of features already present; reconcile its checkboxes only after the corresponding implementation is verified.

## Success criteria

The project is successful when:

1. Every newly initiated upload/download transfers the source version present at execution time, not the version observed when the app or panel opened.
2. A partially transferred file can resume safely after pause, transient failure, or restart when its source fingerprint still matches.
3. No restart triggers unrequested connection or network activity.
4. Conflicts and source changes isolate one job instead of blocking the queue or silently overwriting data.
5. Default concurrency limits and destination serialization hold across every app window.
6. The Transfer Center provides authoritative control and history without duplicating scheduler state in JavaScript.
7. Existing upload/download and editor workflows migrate without losing progress/cancel compatibility.
8. Credentials never enter transfer persistence, events, or logs.
9. Failure recovery preserves an intact old or new destination whenever automatic commit cannot complete.
10. The architecture can add a second transport later without rewriting queue state, UI, or persistence.
