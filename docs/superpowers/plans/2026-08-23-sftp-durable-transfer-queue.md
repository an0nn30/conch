# SFTP Durable Transfer Queue and Transfer Center Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust-owned durable SFTP queue that always opens the latest source at execution time, safely resumes validated partial transfers, survives restart without automatic network activity, and is controlled through a dense Transfer Center tool window.

**Architecture:** `termlab_remote` supplies fresh-open, fingerprint, seek, chunk-copy, and SFTP artifact primitives. A new actor-style queue in `termlab_tauri::remote::transfer_queue` serializes state transitions, persistence, scheduling, retries, connection lookup, and job events. A small frontend transfer runtime projects the authoritative snapshot into a registered Transfer Center; the files panel and editor remain queue producers rather than owning transfer lifecycle state.

**Tech Stack:** Rust 2024, Tokio, Tauri 2, russh/russh-sftp, serde/serde_json, ts-rs, atomic JSON persistence, plain IIFE JavaScript, existing tool-window/tl-dialog/toast/keyboard infrastructure, Node `vm` regression suites.

**Spec:** `docs/superpowers/specs/2026-08-23-sftp-durable-transfer-queue-design.md`

## Global Constraints

- Before Task 1, use `superpowers:using-git-worktrees` to create an isolated worktree from `origin/feat/sftp-durable-transfer-queue`; the current source workspace contains unrelated user changes. Use a repository-compliant `feat/*` implementation branch, never `main`, never force-push, and never add `Co-Authored-By` trailers.
- Every new function/module/behavior gets unit coverage where it can run without a live GUI or server. Follow TDD: red test, smallest implementation, green test, focused commit.
- Every newly dispatched upload/download opens and stats the source during `Checking`, immediately before the first byte. Never cache a source file/SFTP handle or listing metadata across jobs.
- Persist to the platform-aware `termlab_core::config::config_dir().join("transfers.json")`; schema version starts at `1`.
- On restore, no connection, retry, or transfer starts automatically. `Connecting`, `Checking`, and `Running` restore as `Paused`, and the global queue restores suspended until an explicit resume.
- Resume requires a job-owned partial plus an exact size and modification-token match. Equal size alone is insufficient. Changed/unverifiable sources require Restart or Skip.
- Defaults: global concurrency `3`, per-host concurrency `2`, at most `3` total automatic attempts for a transient failure, terminal history cap `500`; same normalized destination is always serialized.
- Passwords, private-key contents, key passphrases, vault secrets, and live connection/session handles never enter JSON, Tauri events, TypeScript projections, error strings, or logs.
- Existing `transfer_upload`, `transfer_download`, `transfer_cancel`, and `transfer-progress` remain compatible during migration. They become queue adapters and continue returning/using the same UUID.
- No SQLite, recursive folder transfer, drag-and-drop, sync, bandwidth limiter, automatic reconnect, FTP/FTPS/SCP adapter, or speculative cross-protocol transport trait in this plan.
- Frontend commands go through focused data-service/runtime modules; no direct `window.__TAURI__.core.invoke`. New CSS uses design-system tokens only and must pass `scripts/check_frontend_boundaries.sh`.
- Do not stage or modify the source workspace's unrelated `Makefile`, packaging files, `icons/`, or currently-untracked `SFTP_FILEZILLA_GAP.md`. Record verified backlog reconciliation in the tracked implementation note instead.

---

## File Structure

### Rust: `termlab_remote`

- `crates/termlab_remote/src/transfer.rs` — keep legacy `TransferProgress` compatibility types and re-export the new focused transfer modules; remove the active-only registry only after Tauri migration is green.
- `crates/termlab_remote/src/transfer/fingerprint.rs` — shared serializable `SourceFingerprint` value type used by primitives and queue persistence.
- `crates/termlab_remote/src/transfer/copy.rs` — cooperative control enum, generic seek/copy loop, checkpoint outcome.
- `crates/termlab_remote/src/transfer/sftp_io.rs` — fresh local/remote fingerprinting, upload/download-to-partial, partial stat/truncate, and local/SFTP artifact operations.

### Rust: `termlab_tauri`

- `crates/termlab_tauri/src/remote/transfer_queue/mod.rs` — public-in-crate queue surface and bootstrap wiring only.
- `crates/termlab_tauri/src/remote/transfer_queue/model.rs` — persisted job/store types, frontend projections, settings, state/reason/result enums.
- `crates/termlab_tauri/src/remote/transfer_queue/reducer.rs` — pure legal transition reducer.
- `crates/termlab_tauri/src/remote/transfer_queue/store.rs` — private atomic JSON writes, migrations, quarantine, permissions, startup recovery.
- `crates/termlab_tauri/src/remote/transfer_queue/scheduler.rs` — pure priority/concurrency/destination selection and retry classification/backoff.
- `crates/termlab_tauri/src/remote/transfer_queue/artifacts.rs` — managed path naming, commit phases, artifact-inventory recovery decisions.
- `crates/termlab_tauri/src/remote/transfer_queue/events.rs` — event-sink abstraction, Tauri event payloads, legacy progress mapping.
- `crates/termlab_tauri/src/remote/transfer_queue/engine.rs` — single-owner queue actor, command protocol, snapshots, persistence barriers, active task controls.
- `crates/termlab_tauri/src/remote/transfer_queue/runner.rs` — runner trait, reporter/ack protocol, real SFTP runner, connection lookup, commit execution.
- `crates/termlab_tauri/src/remote/transfer_commands.rs` — thin Tauri adapters for legacy enqueue plus queue commands.
- `crates/termlab_tauri/src/remote/mod.rs` — register the queue module and enrich live connection identity; remove old registry/channel fields at migration.
- `crates/termlab_tauri/src/lib.rs` — load/manage/start the queue and register commands; remove the old progress-forwarder thread when queue events own it.
- `crates/termlab_tauri/src/cleanup.rs`, `remote/server_commands.rs`, `remote/detached_commands.rs`, `remote/ssh_commands.rs` — update `RemoteState`/`SshConnection` fixtures and constructors for the new queue/endpoint identity.

### Frontend

- `crates/termlab_tauri/frontend/app/features/transfers/data-service.js` — typed command wrapper names and payload construction.
- `crates/termlab_tauri/frontend/app/features/transfers/store.js` — pure snapshot/event reducer with revision-gap detection.
- `crates/termlab_tauri/frontend/app/features/transfers/runtime.js` — one idempotent per-window subscription, commands, subscribers, aggregate summary toasts.
- `crates/termlab_tauri/frontend/app/features/transfers/view.js` — dense table/toolbar renderer and event delegation.
- `crates/termlab_tauri/frontend/app/features/transfers/dialogs.js` — details, conflict/source resolution, cancel, and concurrency dialogs.
- `crates/termlab_tauri/frontend/app/panels/transfer-center.js` — tool-window controller, selection/filter state, keyboard behavior, runtime subscription.
- `crates/termlab_tauri/frontend/styles/design-system/components/transfer-center.css` — token-only dense table, progress, status, focus, empty/error states.
- `crates/termlab_tauri/frontend/app/tool-window-runtime.js`, `frontend/index.html`, `frontend/app/core/commands.js`, `frontend/app/core/tauri-client.js` — registration and script/style/command graph.
- `crates/termlab_tauri/frontend/app/features/files/data-service.js`, `features/files/transfers.js`, `panels/files-panel.js`, `features/editor/editor-service.js` — enqueue origin/conflict intent, badges, refresh, and editor priority integration.

### Tests and docs

- Rust tests stay beside each focused module under `#[cfg(test)]`.
- `scripts/tests/test_transfer_store.mjs` — frontend projection/runtime ordering and revision recovery.
- `scripts/tests/test_transfer_center.mjs` — tool-window rendering, actions, dialogs, keyboard, accessibility, and update isolation.
- Existing `scripts/tests/test_files_transfers.mjs`, `test_editor_remote_transfer.mjs`, and `test_panel_host.mjs` — compatibility and integration updates.
- `docs/superpowers/notes/sftp-transfer-queue-manual-checklist.md` — tracked live-SFTP/restart/UX verification plus backlog reconciliation.

---

### Task 1: Define the persisted job model and legal state machine

**Files:**
- Create: `crates/termlab_remote/src/transfer/fingerprint.rs`
- Modify: `crates/termlab_remote/src/transfer.rs`
- Create: `crates/termlab_tauri/src/remote/transfer_queue/mod.rs`
- Create: `crates/termlab_tauri/src/remote/transfer_queue/model.rs`
- Create: `crates/termlab_tauri/src/remote/transfer_queue/reducer.rs`
- Modify: `crates/termlab_tauri/src/remote/mod.rs`
- Generate: `crates/termlab_tauri/frontend/types/TransferJob.ts` and dependent ts-rs types

**Interfaces:**
- Produces: shared `termlab_remote::transfer::SourceFingerprint`; `TransferQueueDocument`, `TransferJob`, `TransferJobState`, `TransferEndpoint`, `NewTransferJob`, `TransferQueueSnapshot`, `TransferQueueSummary`, `QueueSettings`, `ManagedArtifacts`, `CommitPhase`, `ConflictPolicy`, `ConflictResolution`, `JobEvent`, and `reduce_job(&TransferJob, JobEvent, u64) -> Result<TransferJob, TransitionError>`.
- Later tasks must use `TransferJobState`'s internally tagged camelCase shape (`job.state.kind`) and must not invent parallel boolean state flags.

- [ ] **Step 1: Write failing model serialization/default tests**

Add tests proving version/default values, camelCase tags, the history/queue distinction, and the absence of credential-shaped fields:

```rust
#[test]
fn document_defaults_are_versioned_and_suspended() {
    let doc = TransferQueueDocument::default();
    assert_eq!(doc.version, TRANSFER_STORE_VERSION);
    assert!(doc.queue_paused);
    assert_eq!(doc.settings, QueueSettings { global_limit: 3, per_host_limit: 2 });
    assert!(doc.jobs.is_empty());
}

#[test]
fn serialized_job_uses_tagged_camel_case_without_secrets() {
    let json = serde_json::to_string(&sample_job(TransferJobState::NeedsConnection {
        message: "Reconnect explicitly".into(),
    })).unwrap();
    assert!(json.contains("needsConnection"));
    for forbidden in ["password", "passphrase", "privateKey", "sshHandle", "sessionHandle"] {
        assert!(!json.contains(forbidden), "serialized job leaked field {forbidden}");
    }
}
```

- [ ] **Step 2: Run the focused tests and verify the module is missing**

Run: `cargo test -p termlab_tauri remote::transfer_queue::model::tests -- --nocapture`

Expected: FAIL because `remote::transfer_queue` and its types do not exist.

- [ ] **Step 3: Implement the exact domain shapes**

Define the shared fingerprint in `termlab_remote` and use that exact type in the Tauri record:

```rust
// termlab_remote/src/transfer/fingerprint.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceFingerprint {
    #[ts(as = "f64")]
    pub size: u64,
    pub modified_token: Option<String>,
}

// termlab_tauri/src/remote/transfer_queue/model.rs
pub const TRANSFER_STORE_VERSION: u32 = 1;
pub const TRANSFER_HISTORY_LIMIT: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
#[ts(export)]
pub enum TransferEndpoint {
    Configured { server_entry_id: String, label: String },
    AdHoc {
        host: String,
        port: u16,
        user: String,
        proxy_command: Option<String>,
        proxy_jump: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferProtocol { Sftp }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferDirection { Upload, Download }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
#[ts(export)]
pub enum TransferOrigin {
    FilesPanel,
    Editor,
    Other { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferPriority { Interactive, Normal }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum ConflictPolicy { Ask, Overwrite }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
#[ts(export)]
pub enum ConflictResolution {
    Resume,
    Overwrite,
    Rename { destination: String },
    Skip,
    Restart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
#[ts(export)]
pub enum AttentionReason {
    DestinationConflict { resume_available: bool },
    SourceChanged { expected: SourceFingerprint, actual: SourceFingerprint },
    SourceCannotResume,
    MissingPartial,
    CommitRecovery { message: String },
    Cleanup { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CompletionResult { Transferred, Skipped }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CommitPhase { None, Prepared, BackupMoved, PartialPromoted, CleanupPending, Complete }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagedArtifacts {
    pub partial_path: String,
    pub backup_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
#[ts(export)]
pub enum TransferJobState {
    Queued,
    Connecting,
    Checking,
    Running,
    Paused,
    NeedsConnection { message: String },
    NeedsAttention { reason: AttentionReason },
    RetryWaiting { attempt: u8, #[ts(as = "f64")] next_retry_at_ms: u64 },
    Completed { result: CompletionResult },
    Failed { error: String },
    Cancelled { cleanup_error: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueSettings {
    pub global_limit: usize,
    pub per_host_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferJob {
    pub id: Uuid,
    pub protocol: TransferProtocol,
    pub direction: TransferDirection,
    pub origin: TransferOrigin,
    pub endpoint: TransferEndpoint,
    pub local_path: String,
    pub remote_path: String,
    pub file_name: String,
    pub batch_id: Option<Uuid>,
    pub priority: TransferPriority,
    #[ts(as = "f64")]
    pub queue_order: u64,
    pub host_key: String,
    pub destination_key: String,
    pub state: TransferJobState,
    pub source_fingerprint: Option<SourceFingerprint>,
    #[ts(as = "f64")]
    pub durable_checkpoint: u64,
    #[ts(as = "f64")]
    pub bytes_transferred: u64,
    #[ts(as = "f64")]
    pub total_bytes: u64,
    #[ts(as = "f64")]
    pub speed_bytes_per_second: u64,
    #[ts(as = "Option<f64>")]
    pub eta_seconds: Option<u64>,
    pub retry_attempt: u8,
    pub max_attempts: u8,
    pub conflict_policy: ConflictPolicy,
    pub artifacts: Option<ManagedArtifacts>,
    pub commit_phase: CommitPhase,
    #[ts(as = "f64")]
    pub created_at_ms: u64,
    #[ts(as = "f64")]
    pub updated_at_ms: u64,
    #[ts(as = "Option<f64>")]
    pub started_at_ms: Option<u64>,
    #[ts(as = "Option<f64>")]
    pub finished_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferQueueDocument {
    pub version: u32,
    pub revision: u64,
    pub queue_paused: bool,
    pub settings: QueueSettings,
    pub jobs: Vec<TransferJob>,
    pub recovery_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewTransferJob {
    pub id: Uuid,
    pub protocol: TransferProtocol,
    pub direction: TransferDirection,
    pub origin: TransferOrigin,
    pub endpoint: TransferEndpoint,
    pub local_path: String,
    pub remote_path: String,
    pub file_name: String,
    pub batch_id: Option<Uuid>,
    pub conflict_policy: ConflictPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferQueueSummary {
    pub queued: usize,
    pub running: usize,
    pub paused: usize,
    pub attention: usize,
    pub failed: usize,
    pub active: usize,
    pub history: usize,
    pub queue_paused: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferQueueSnapshot {
    #[ts(as = "f64")]
    pub revision: u64,
    pub queue_paused: bool,
    pub settings: QueueSettings,
    pub jobs: Vec<TransferJob>,
    pub summary: TransferQueueSummary,
    pub recovery_error: Option<String>,
}

pub enum JobEvent {
    BeginConnect,
    BeginCheck,
    BeginRun {
        fingerprint: SourceFingerprint,
        total_bytes: u64,
        artifacts: ManagedArtifacts,
    },
    Progress { bytes: u64, speed_bytes_per_second: u64, eta_seconds: Option<u64> },
    Checkpoint { bytes: u64 },
    Pause,
    Resume,
    NeedsConnection(String),
    NeedsAttention(AttentionReason),
    RetryScheduled { attempt: u8, next_retry_at_ms: u64 },
    RetryReady,
    ManualRetry,
    Resolve(ConflictResolution),
    Restart,
    Complete(CompletionResult),
    Fail(String),
    Cancel(Option<String>),
}
```

The reducer initializes state/order/timestamps/checkpoint/retry/artifact fields that are absent from `NewTransferJob`. Modification tokens are backend-generated (`unixNs:<value>` locally and `unixSeconds:<value>` remotely), so equality is exact without unsafe JavaScript integer conversion. `max_attempts` defaults to `3`; persisted checkpoint remains separate from display progress. `transfer.rs` declares `mod fingerprint; pub use fingerprint::SourceFingerprint;` so both crates use one definition.

- [ ] **Step 4: Write failing reducer tests for every approved branch**

Table-test at least these transitions: queued→connecting→checking→running→completed; running→paused→queued; checking→needs-connection; checking/running→needs-attention; transient failure→retry-waiting→queued; restart clears fingerprint/checkpoint/artifacts; terminal states reject further progress; illegal queued→completed is rejected.

```rust
#[test]
fn restart_after_source_change_clears_attempt_identity() {
    let job = sample_job_with_checkpoint(4096, "unixNs:12");
    let attention = reduce_job(&job, JobEvent::NeedsAttention(source_changed()), 50).unwrap();
    let restarted = reduce_job(&attention, JobEvent::Restart, 51).unwrap();
    assert_eq!(restarted.state, TransferJobState::Queued);
    assert_eq!(restarted.durable_checkpoint, 0);
    assert_eq!(restarted.source_fingerprint, None);
    assert_eq!(restarted.artifacts, None);
}
```

- [ ] **Step 5: Implement the pure reducer and make both suites green**

Run: `cargo test -p termlab_tauri remote::transfer_queue -- --nocapture`

Expected: PASS; ts-rs writes the new frontend type files and all enum/property names match the Rust serde representation.

- [ ] **Step 6: Commit the independently reviewable model**

```bash
git add crates/termlab_remote/src/transfer.rs crates/termlab_remote/src/transfer/fingerprint.rs crates/termlab_tauri/src/remote/transfer_queue crates/termlab_tauri/src/remote/mod.rs crates/termlab_tauri/frontend/types
git commit -m "Add durable transfer job state model"
```

---

### Task 2: Persist, quarantine, migrate, and recover queue state

**Files:**
- Create: `crates/termlab_tauri/src/remote/transfer_queue/store.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/mod.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/model.rs`

**Interfaces:**
- Consumes: Task 1's `TransferQueueDocument` and state enums.
- Produces: `TransferStore::new(PathBuf)`, `load() -> Result<LoadOutcome, StoreError>`, `save(&TransferQueueDocument)`, `recover_for_startup(&mut TransferQueueDocument)`, and `LoadOutcome::{Loaded, Quarantined}`.

- [ ] **Step 1: Write failing atomic round-trip and permissions tests**

```rust
#[test]
fn save_roundtrips_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transfers.json");
    let store = TransferStore::new(path.clone());
    store.save(&document_with(sample_job(TransferJobState::Paused))).unwrap();
    let loaded = store.load().unwrap().into_document();
    assert_eq!(loaded.jobs.len(), 1);
    assert!(std::fs::read_dir(dir.path()).unwrap()
        .all(|entry| !entry.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
}

#[cfg(unix)]
#[test]
fn new_store_is_user_only() {
    use std::os::unix::fs::PermissionsExt;
    let (store, path) = temp_store();
    store.save(&TransferQueueDocument::default()).unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
}
```

- [ ] **Step 2: Run the store tests and verify they fail**

Run: `cargo test -p termlab_tauri remote::transfer_queue::store::tests -- --nocapture`

Expected: FAIL because `TransferStore` does not exist.

- [ ] **Step 3: Implement private atomic writes and schema loading**

Write a uniquely named sibling temporary file with `OpenOptions`, mode `0o600` on Unix, `write_all`, `flush`, and `sync_all`; rename it over the target and sync the parent directory where supported. Never share a fixed `.tmp` filename. `save` serializes pretty JSON only after in-memory validation has succeeded.

```rust
pub struct TransferStore { path: PathBuf }

impl TransferStore {
    pub fn new(path: PathBuf) -> Self { Self { path } }
    pub fn save(&self, document: &TransferQueueDocument) -> Result<(), StoreError>;
    pub fn load(&self) -> Result<LoadOutcome, StoreError>;
}
```

Accept version `1`; reject version `0` or future versions with a typed migration error. An absent file loads `TransferQueueDocument::default()` and does not create it until the first mutation.

- [ ] **Step 4: Write failing quarantine and startup-recovery tests**

Cover malformed JSON renamed to `transfers.corrupt-<unix-ms>.json`; original bytes preserved; a fresh suspended document returned with `recovery_error` naming the quarantine file. Cover `Connecting`, `Checking`, and `Running` becoming `Paused`; `Queued`, `Paused`, `NeedsConnection`, `NeedsAttention`, and `RetryWaiting` retaining their job state; queue forced suspended; no active job evicted; only terminal history over 500 compacted oldest-first.

- [ ] **Step 5: Implement quarantine, recovery, and compaction**

`recover_for_startup` is pure and runs after deserialize/migration, before the document is published. It clears runtime-only display speed/ETA, retains managed partial/checkpoint/commit data, forces `queue_paused = true`, and increments the revision once if it changed anything.

Run: `cargo test -p termlab_tauri remote::transfer_queue::store::tests -- --nocapture`

Expected: PASS, including malformed-file byte preservation and history cap.

- [ ] **Step 6: Commit persistence independently**

```bash
git add crates/termlab_tauri/src/remote/transfer_queue
git commit -m "Persist transfer queue state safely"
```

---

### Task 3: Add deterministic scheduling, destination locks, and retry policy

**Files:**
- Create: `crates/termlab_tauri/src/remote/transfer_queue/scheduler.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/mod.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/model.rs`

**Interfaces:**
- Consumes: Task 1 model and Task 2 restored/suspended document.
- Produces: `select_runnable_jobs`, `ActiveLease`, `FailureClass`, `classify_failure`, and `retry_delay_ms`.

- [ ] **Step 1: Write failing scheduler tests**

Build jobs with explicit host keys/destination keys and assert:

```rust
#[test]
fn selection_honors_priority_limits_and_destination_serialization() {
    let settings = QueueSettings { global_limit: 3, per_host_limit: 2 };
    let jobs = vec![
        queued("normal-a", Normal, "host-a", "/same"),
        queued("editor", Interactive, "host-a", "/editor"),
        queued("same-destination", Normal, "host-b", "/same"),
        queued("host-cap", Normal, "host-a", "/third"),
        queued("other-host", Normal, "host-b", "/other"),
    ];
    let chosen = select_runnable_jobs(&jobs, &[], &settings, 100);
    assert_eq!(chosen, ids(["editor", "normal-a", "other-host"]));
}
```

Also cover explicit user order before creation time, blocked/attention jobs skipped, retry-waiting before deadline skipped, due retry eligible, active Connecting/Checking/Running leases counted, and queue-paused returns no jobs.

- [ ] **Step 2: Verify scheduler tests fail**

Run: `cargo test -p termlab_tauri remote::transfer_queue::scheduler::tests -- --nocapture`

Expected: FAIL because scheduler functions are undefined.

- [ ] **Step 3: Implement stable selection and normalized lease accounting**

```rust
pub struct ActiveLease {
    pub job_id: Uuid,
    pub host_key: String,
    pub destination_key: String,
}

pub fn select_runnable_jobs(
    jobs: &[TransferJob],
    active: &[ActiveLease],
    settings: &QueueSettings,
    now_ms: u64,
) -> Vec<Uuid>;
```

Sort by priority (`Interactive` first), then `queue_order`, then `created_at_ms`; walk once, reserving global/host/destination capacity as each job is chosen. Never reorder the stored vector to compute selection.

- [ ] **Step 4: Write and implement retry-classification tests**

Map disconnect/time-out/connection-reset to `Transient`; authentication, permissions, source mismatch, conflict, missing partial, and commit ambiguity to `Permanent`. Attempt `1` is the initial run; `retry_delay_ms(2)` returns `1_000` and `retry_delay_ms(3)` returns `2_000`. A failure on attempt `3` is exhausted and must not schedule attempt `4`.

Run: `cargo test -p termlab_tauri remote::transfer_queue::scheduler::tests -- --nocapture`

Expected: PASS with no sleeps or wall-clock dependency.

- [ ] **Step 5: Commit scheduling policy**

```bash
git add crates/termlab_tauri/src/remote/transfer_queue
git commit -m "Add deterministic transfer scheduling"
```

---

### Task 4: Add fresh-open fingerprints and resumable chunk-copy primitives

**Files:**
- Create: `crates/termlab_remote/src/transfer/copy.rs`
- Create: `crates/termlab_remote/src/transfer/sftp_io.rs`
- Modify: `crates/termlab_remote/src/transfer.rs`

**Interfaces:**
- Consumes: Task 1's shared `termlab_remote::transfer::SourceFingerprint`.
- Produces: `ControlDecision`, `CopyOutcome`, `copy_with_checkpoint`, `fingerprint_open_local`, `fingerprint_open_remote`, `upload_to_partial`, `download_to_partial`, `truncate_local_partial`, `truncate_remote_partial`, and focused local/SFTP artifact helpers.
- Task 8's real runner is the only policy caller. These functions perform I/O but never decide queue state, conflict policy, retry policy, or authentication.

- [ ] **Step 1: Write failing generic copy-loop tests**

Use in-memory async cursors and a small test chunk size to prove: both sides seek to the supplied offset; progress reports absolute bytes; Pause stops after the current chunk; Cancel stops without reading a later chunk; EOF returns Completed; read/write error returns `RemoteError::Transfer`.

```rust
#[tokio::test]
async fn resume_seeks_both_streams_and_reports_absolute_checkpoint() {
    let mut source = Cursor::new(b"0123456789".to_vec());
    let mut destination = Cursor::new(b"0123xxxxxx".to_vec());
    let mut seen = Vec::new();
    let outcome = copy_with_checkpoint(
        &mut source, &mut destination, 4, 10, 2,
        || ControlDecision::Continue,
        |done, total| seen.push((done, total)),
    ).await.unwrap();
    assert_eq!(outcome, CopyOutcome::Completed { bytes: 10 });
    assert_eq!(destination.into_inner(), b"0123456789");
    assert_eq!(seen.last(), Some(&(10, 10)));
}
```

- [ ] **Step 2: Run the focused tests and verify missing symbols**

Run: `cargo test -p termlab_remote transfer::copy::tests -- --nocapture`

Expected: FAIL because the submodule and types do not exist.

- [ ] **Step 3: Implement the generic cooperative loop**

Use Tokio `AsyncRead + AsyncSeek` and `AsyncWrite + AsyncSeek`, bounded chunks, and explicit absolute offsets. The loop does not abort tasks: it returns `Paused { bytes }` or `Cancelled { bytes }` after the current chunk so the high-level wrapper can flush/close and report a durable checkpoint.

- [ ] **Step 4: Write failing local/remote fingerprint and partial tests**

Local tests exercise the pure metadata-to-token conversion with equal sizes/different nanosecond mtimes, then use a temp file to prove the opened file's metadata supplies the fingerprint. Partial tests prove a resume open does not truncate, explicit reconciliation truncates to the durable checkpoint, and a fresh open starts at zero.

For remote metadata, unit-test the pure conversion from SFTP `(size, Option<mtime-seconds>)` to `SourceFingerprint`; a missing mtime yields `modified_token = None`, which later disables Resume.

- [ ] **Step 5: Implement fresh-open upload/download wrappers**

`upload_to_partial` opens the local source inside the function on every invocation, fingerprints that open handle, awaits an `on_fingerprint(SourceFingerprint)` acknowledgement callback, and only then reads its first byte. `download_to_partial` does the same with a freshly opened remote source and `File::metadata()`. This callback is the persistence barrier Task 8 connects to `RunnerReporter::fingerprinted`, so a stat/open race cannot associate bytes with older metadata. Remote resume uses `OpenFlags::CREATE | OpenFlags::WRITE` without `TRUNCATE`, followed by seek; fresh transfer creates/truncates. Every remote writer calls `flush()` then `shutdown()`; every remote reader calls `shutdown()`; local destination calls `flush()` and `sync_all()` at checkpoint/finalization.

Run: `cargo test -p termlab_remote transfer -- --nocapture`

Expected: PASS, including the pre-existing flush-then-shutdown freshness tests.

- [ ] **Step 6: Commit the reusable SFTP primitives**

```bash
git add crates/termlab_remote/src/transfer.rs crates/termlab_remote/src/transfer
git commit -m "Add resumable SFTP transfer primitives"
```

---

### Task 5: Model managed artifacts and crash-recoverable commits

**Files:**
- Create: `crates/termlab_tauri/src/remote/transfer_queue/artifacts.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/model.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/mod.rs`

**Interfaces:**
- Consumes: Task 1's `ManagedArtifacts`, `CommitPhase`, and job fields.
- Produces: `ManagedArtifacts::for_destination`, `ArtifactInventory`, `RecoveryAction`, and `recovery_action(CommitPhase, ArtifactInventory)`.
- Task 8 executes the returned action through Task 4 I/O helpers and reports every phase through Task 7's persistence barrier.

- [ ] **Step 1: Write failing managed-name tests**

```rust
#[test]
fn artifacts_are_unique_hidden_siblings_of_the_destination() {
    let id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let a = ManagedArtifacts::for_destination(id, "/srv/releases/app.tar").unwrap();
    assert_eq!(a.partial_path, "/srv/releases/.app.tar.termlab-part-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
    assert_eq!(a.backup_path, "/srv/releases/.app.tar.termlab-backup-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
    assert_ne!(a.partial_path, a.backup_path);
}
```

Cover `/file`, trailing/invalid paths, local platform separators through a separate `for_local_destination(&Path)` helper, and two UUIDs producing different names.

- [ ] **Step 2: Run artifact tests and verify failure**

Run: `cargo test -p termlab_tauri remote::transfer_queue::artifacts::tests -- --nocapture`

Expected: FAIL because the module does not exist.

- [ ] **Step 3: Implement deterministic job-owned artifact naming**

Managed names must always live beside the final destination. Never accept a caller-provided partial/backup path, never derive one outside the destination directory, and never classify an arbitrary `.part` file as TermLab-owned.

- [ ] **Step 4: Write the full commit-recovery matrix before implementation**

Table-test at least:

| Persisted phase | Final | Partial | Backup | Expected action |
|---|---:|---:|---:|---|
| `None` | any | yes | no | `ResumeCopy` |
| `Prepared` | yes | yes | no | `MoveFinalToBackup` |
| `BackupMoved` | no | yes | yes | `PromotePartial` |
| `BackupMoved` | no | no | yes | `RestoreBackup` |
| `PartialPromoted` | yes | no | yes | `DeleteBackupAndComplete` |
| `PartialPromoted` | no | no | yes | `RestoreBackup` |
| any ambiguous combination | mixed | mixed | mixed | `NeedsAttention` |

- [ ] **Step 5: Implement pure recovery decisions and make the matrix green**

Use these phases exactly: `None`, `Prepared`, `BackupMoved`, `PartialPromoted`, `CleanupPending`, `Complete`. `RecoveryAction` must be descriptive and non-destructive; only Task 8 performs I/O. When the matrix cannot prove which artifact is authoritative, return `NeedsAttention { message }` and preserve all artifacts.

Run: `cargo test -p termlab_tauri remote::transfer_queue::artifacts::tests -- --nocapture`

Expected: PASS for every row and ambiguity case.

- [ ] **Step 6: Commit artifact safety policy**

```bash
git add crates/termlab_tauri/src/remote/transfer_queue
git commit -m "Add recoverable transfer artifact commits"
```

---

### Task 6: Build the single-owner queue actor and command protocol

**Files:**
- Create: `crates/termlab_tauri/src/remote/transfer_queue/events.rs`
- Create: `crates/termlab_tauri/src/remote/transfer_queue/engine.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/mod.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/model.rs`

**Interfaces:**
- Consumes: reducer, store, scheduler, and artifact model from Tasks 1-5.
- Produces: `TransferQueueHandle`, `QueueActor`, `QueueCommand`, `TransferEventSink`, `QueueEventPayload`, `QueueSummaryPayload`, and async control methods used by Tauri commands and the scheduler runtime.
- Task 7 adds runner dispatch to this actor without changing frontend-facing method names.

- [ ] **Step 1: Write failing actor tests with an in-memory event sink**

Create `RecordingEventSink` and a temp store. Prove enqueue returns/stores the supplied UUID, increments revision, persists before the first job event, and snapshot equals the stored document. Prove two independent handles observe the same actor snapshot.

```rust
#[tokio::test]
async fn enqueue_persists_before_job_event_is_published() {
    let harness = ActorHarness::new();
    let id = harness.handle.enqueue(sample_new_job()).await.unwrap();
    let event = harness.events.take_job_update(id);
    let on_disk = harness.store.load().unwrap().into_document();
    assert_eq!(on_disk.revision, event.revision);
    assert!(on_disk.jobs.iter().any(|job| job.id == id));
}
```

- [ ] **Step 2: Run actor tests and verify missing types**

Run: `cargo test -p termlab_tauri remote::transfer_queue::engine::tests -- --nocapture`

Expected: FAIL because `TransferQueueHandle`/actor do not exist.

- [ ] **Step 3: Implement the command channel, snapshots, and event sink**

Use one Tokio unbounded command channel and oneshot replies. The actor exclusively owns `TransferQueueDocument`; `TransferQueueHandle` holds only the sender and an `Arc<RwLock<TransferQueueSnapshot>>` read model.

```rust
#[derive(Clone)]
pub struct TransferQueueHandle {
    command_tx: mpsc::UnboundedSender<QueueCommand>,
    snapshot: Arc<parking_lot::RwLock<TransferQueueSnapshot>>,
}

#[async_trait::async_trait]
pub trait TransferEventSink: Send + Sync {
    async fn job_updated(&self, payload: QueueEventPayload);
    async fn queue_summary(&self, payload: QueueSummaryPayload);
    async fn legacy_progress(&self, payload: termlab_remote::transfer::TransferProgress);
}
```

Every mutating command follows one function: reduce/mutate → compact/validate → store.save → update snapshot → emit. If save fails, leave the published snapshot/revision unchanged and return the error.

- [ ] **Step 4: Add failing command behavior tests**

Cover pause/resume, Pause All/Resume All, cancel cleanup intent, retry, Restart/Skip/Overwrite/Rename resolution validation, reorder, priority change, clear completed, queue settings validation (`1..=32` global and per-host; per-host may exceed global but effective scheduling still cannot), and illegal actions returning an error without revision/event changes.

- [ ] **Step 5: Implement the stable handle surface**

The exact methods later Tauri wrappers call are:

```rust
pub async fn enqueue(&self, request: NewTransferJob) -> Result<Uuid, String>;
pub fn snapshot(&self) -> TransferQueueSnapshot;
pub async fn pause(&self, id: Uuid) -> Result<(), String>;
pub async fn resume(&self, id: Uuid) -> Result<(), String>;
pub async fn pause_all(&self) -> Result<(), String>;
pub async fn resume_all(&self) -> Result<(), String>;
pub async fn cancel(&self, id: Uuid) -> Result<bool, String>;
pub async fn retry(&self, id: Uuid) -> Result<(), String>;
pub async fn resolve(&self, id: Uuid, resolution: ConflictResolution) -> Result<(), String>;
pub async fn reorder(&self, id: Uuid, before: Option<Uuid>) -> Result<(), String>;
pub async fn set_priority(&self, id: Uuid, priority: TransferPriority) -> Result<(), String>;
pub async fn clear_completed(&self) -> Result<usize, String>;
pub async fn update_settings(&self, settings: QueueSettings) -> Result<(), String>;
```

Run: `cargo test -p termlab_tauri remote::transfer_queue::engine::tests -- --nocapture`

Expected: PASS with persisted revision matching every emitted revision.

- [ ] **Step 6: Commit the actor before adding background work**

```bash
git add crates/termlab_tauri/src/remote/transfer_queue
git commit -m "Add the durable transfer queue actor"
```

---

### Task 7: Dispatch runners with pause/cancel/checkpoint barriers and deterministic retries

**Files:**
- Create: `crates/termlab_tauri/src/remote/transfer_queue/runner.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/engine.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/events.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/mod.rs`

**Interfaces:**
- Consumes: Task 3 selection/retry helpers and Task 6 actor.
- Produces: `TransferJobRunner`, `RunnerControl`, `RunnerReporter`, `RunnerResult`, `RunnerEvent`, `QueueClock`, and `SystemQueueClock`.
- Task 8 implements `TransferJobRunner` for real SFTP; tests here use only deterministic fakes.

- [ ] **Step 1: Write failing dispatch tests using a gated fake runner and fake clock**

The fake runner records starts and waits on test-controlled gates. Prove no task starts while restored/global pause is active; explicit Resume All starts at most 3 globally/2 per host; a completed task releases both limits and destination lock; same destination never overlaps; interactive job starts first; a blocked job does not prevent later jobs.

- [ ] **Step 2: Define runner/reporting interfaces and verify tests still fail**

```rust
#[async_trait::async_trait]
pub trait TransferJobRunner: Send + Sync {
    async fn run(
        &self,
        job: TransferJob,
        control: RunnerControl,
        reporter: RunnerReporter,
    ) -> RunnerResult;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerControlState { Run, Pause, Cancel }

pub enum RunnerResult {
    Completed(CompletionResult),
    Paused { durable_checkpoint: u64 },
    Cancelled { cleanup_error: Option<String> },
    NeedsConnection(String),
    NeedsAttention(AttentionReason),
    Failed { class: FailureClass, message: String },
}
```

`RunnerReporter::fingerprinted`, `durable_checkpoint`, and `commit_phase` send an actor event with a oneshot acknowledgement. The runner must await `Ok(())` before reading more bytes or performing the next commit mutation. `progress` is fire-and-forget and coalescible.

- [ ] **Step 3: Implement scheduling/wake/task-control integration**

The actor runs selection after every relevant command, runner result, acknowledged checkpoint, and retry wake. Active tasks hold `watch::Sender<RunnerControlState>` plus their `ActiveLease`. Pause changes the sender to `Pause`; Cancel changes it to `Cancel`; neither uses `AbortHandle::abort` during normal copying. Once `CommitPhase::Prepared` is acknowledged, pause/cancel is deferred until promotion/rollback reaches a safe recoverable state; control requests must never interrupt the old-final/backup swap.

- [ ] **Step 4: Write failing checkpoint/retry tests**

Prove displayed bytes may advance internally without a store write/event until the coalescing interval, but an acknowledged durable checkpoint is on disk before the fake runner is released. Prove Pause waits for the fake runner's checkpoint/result and only then publishes `Paused`. Prove transient failures schedule persisted 1s/2s deadlines for attempts 2/3 with a fake clock, permanent failures go directly to `Failed`, and attempt 3 is terminal.

- [ ] **Step 5: Implement the fakeable clock and retry wake**

```rust
#[async_trait::async_trait]
pub trait QueueClock: Send + Sync {
    fn now_ms(&self) -> u64;
    async fn sleep_until(&self, unix_ms: u64);
}
```

Production `SystemQueueClock` uses `tokio::time::sleep`; test clock advances only when the test calls `advance_to`. Waking a paused queue must not dispatch due retries until explicit resume.

Run: `cargo test -p termlab_tauri remote::transfer_queue::engine::tests -- --nocapture`

Expected: PASS without real sleeps.

- [ ] **Step 6: Commit background dispatch mechanics**

```bash
git add crates/termlab_tauri/src/remote/transfer_queue
git commit -m "Dispatch queued transfers with durable checkpoints"
```

---

### Task 8: Implement the real SFTP runner, connection lookup, conflict handling, and commit execution

**Files:**
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/runner.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/artifacts.rs`
- Modify: `crates/termlab_tauri/src/remote/mod.rs`
- Modify: `crates/termlab_tauri/src/remote/ssh_commands.rs`
- Modify: `crates/termlab_tauri/src/remote/detached_commands.rs`
- Modify: `crates/termlab_tauri/src/remote/server_commands.rs`

**Interfaces:**
- Consumes: Task 4 I/O primitives, Task 5 recovery actions, Task 7 runner/reporting API, existing live `RemoteState.sessions/connections`.
- Produces: `SftpTransferJobRunner` and `resolve_live_sftp_connection(&RemoteState, &TransferEndpoint) -> Option<ResolvedSftpConnection>`.

- [ ] **Step 1: Write failing connection-identity tests**

Extend `SshConnection` with non-secret `server_entry_id`, `proxy_command`, and `proxy_jump`. Test configured lookup matches exact server ID; ad-hoc lookup matches host+port+user; wrong user/port does not bind; no match returns None; no serialized queue/event type can hold the returned SSH handle.

- [ ] **Step 2: Implement connection identity at every constructor**

`establish_ssh_session` and detached registration copy the server ID/proxy fields into `SshConnection`; shared-channel constructors inherit them from the existing connection. Keep `SshSession.server_entry_id` semantics unchanged so detached duplicate detection is not broadened to terminal sessions.

Run: `cargo test -p termlab_tauri remote:: -- --nocapture`

Expected: PASS with all existing connection/session tests updated through focused fixture builders.

- [ ] **Step 3: Write failing real-runner tests around local/temp I/O seams**

With a fake connection resolver and fake SFTP operations, prove the execution order:

1. resolve live connection or return `NeedsConnection` without I/O;
2. transition/report `Checking`;
3. open/stat the current source;
4. compare/freeze fingerprint;
5. inspect final/partial and return typed attention when required;
6. reconcile partial to the acknowledged checkpoint;
7. copy cooperatively;
8. checkpoint/close;
9. commit through acknowledged phases;
10. return Completed.

The I/O fake is a test seam around local/SFTP artifact operations, not a cross-protocol adapter. Keep it private to `runner.rs` and name it `TransferIo` rather than `Transport`.

- [ ] **Step 4: Implement source validation and conflict decisions**

A new attempt fingerprints during `Checking`; a resumed/retried attempt requires exact fingerprint equality. `modified_token = None` allows a fresh full copy but returns `NeedsAttention(SourceCannotResume)` after interruption. An existing arbitrary final with policy `Ask` returns `DestinationConflict`; policy `Overwrite` leaves the old final in place until commit. The `Rename` resolution updates the direction-appropriate final path and destination-lock identity before redispatch; `Skip` returns `Completed(Skipped)` without I/O. Resume is offered only for a matching job-owned partial/checkpoint.

- [ ] **Step 5: Implement partial reconciliation and recoverable commit**

Before resume, stat the partial. If it is longer than the durable checkpoint, truncate to the checkpoint. If shorter, use the actual shorter size only after persisting the lowered checkpoint; if missing/incompatible, return attention. Commit uses these acknowledged phase barriers:

```text
Prepared (persist) -> move final to backup -> BackupMoved (persist)
-> move partial to final -> PartialPromoted (persist)
-> delete backup -> Complete (persist)
```

If promotion fails, attempt backup restore and report the resulting artifact inventory. Never delete the only intact final/backup/partial in an ambiguous state.

- [ ] **Step 6: Add runner outcome/legacy-progress tests**

Prove pause preserves the partial, cancel deletes only exact job-owned artifacts, disk-full/permission errors preserve checkpoint/artifacts and classify permanent, transient disconnect classifies retryable, and Completed is emitted only after flush/close and commit completion.

Run: `cargo test -p termlab_tauri remote::transfer_queue::runner::tests -- --nocapture`

Expected: PASS; `cargo test -p termlab_remote transfer -- --nocapture` remains green.

- [ ] **Step 7: Commit the real SFTP execution path**

```bash
git add crates/termlab_tauri/src/remote crates/termlab_remote/src/transfer.rs crates/termlab_remote/src/transfer
git commit -m "Run durable queue jobs through resumable SFTP"
```

---

### Task 9: Wire startup, Tauri commands/events, and legacy compatibility

**Files:**
- Modify: `crates/termlab_tauri/src/remote/transfer_commands.rs`
- Modify: `crates/termlab_tauri/src/remote/mod.rs`
- Modify: `crates/termlab_tauri/src/lib.rs`
- Modify: `crates/termlab_tauri/src/cleanup.rs`
- Modify: `crates/termlab_tauri/src/remote/server_commands.rs`
- Modify: `crates/termlab_remote/src/transfer.rs`
- Generate: new/updated files under `crates/termlab_tauri/frontend/types/`

**Interfaces:**
- Consumes: Tasks 6-8 queue handle/runner and existing Tauri caller→parent session resolver.
- Produces exact commands: `transfer_queue_snapshot`, `transfer_pause`, `transfer_resume`, `transfer_pause_all`, `transfer_resume_all`, `transfer_cancel`, `transfer_retry`, `transfer_resolve`, `transfer_reorder`, `transfer_set_priority`, `transfer_clear_completed`, `transfer_update_settings`, plus compatible `transfer_upload`/`transfer_download`.
- Produces exact events: `transfer-job-updated`, `transfer-queue-summary`, and legacy `transfer-progress`.

- [ ] **Step 1: Write failing command request-construction tests**

Extract a pure helper that resolves the caller's parent label, pane/session/connection, configured-or-ad-hoc endpoint, normalized destination key, origin, priority, and conflict policy. Test configured detached session; terminal/ad-hoc session with proxy snapshot; panel-host caller resolving to parent; missing session error; editor origin→Interactive; omitted origin→FilesPanel/Normal.

- [ ] **Step 2: Change legacy enqueue signatures without breaking omitted arguments**

```rust
#[tauri::command]
pub(crate) async fn transfer_upload(
    window: tauri::WebviewWindow,
    remote: tauri::State<'_, Arc<Mutex<RemoteState>>>,
    queue: tauri::State<'_, TransferQueueHandle>,
    pane_id: u32,
    local_path: String,
    remote_path: String,
    origin: Option<TransferOrigin>,
    conflict_policy: Option<ConflictPolicy>,
) -> Result<String, String>;
```

`transfer_download` mirrors it. Mint the UUID before enqueue, put the same UUID in `NewTransferJob`, and return it as `String`. Omitted origin/policy remain valid for old frontend callers. Keep `transfer_cancel(transfer_id: String) -> bool`; it delegates to the actor and returns false for unknown IDs.

- [ ] **Step 3: Write failing event-sink compatibility tests**

Prove new payloads include monotonically increasing revision and a full updated job/removal marker; summary includes active/history/attention/failed counts and queue paused state. Prove Running progress maps to legacy `in_progress`, Completed to `completed`, Failed to `failed`, Cancelled to `cancelled`, with the same UUID/direction/file-name/byte fields. NeedsConnection/NeedsAttention are only new queue states, not falsely reported as legacy terminal completion.

- [ ] **Step 4: Bootstrap the queue from the application config directory**

Before `Builder`, load `config_dir().join("transfers.json")` into a queue bootstrap/handle. `.manage(queue_handle.clone())`; in `.setup`, start the actor with `TauriTransferEventSink`, `SftpTransferJobRunner`, `SystemQueueClock`, and `Arc<Mutex<RemoteState>>`. A quarantined-store error remains in the snapshot so late frontend subscribers see it. Do not emit early-only recovery events.

- [ ] **Step 5: Register all queue commands and remove the active-only channel/registry**

Delete `RemoteState.transfers`, `transfer_progress_tx`, the `transfer_rx` forwarder thread, `TransferRegistry`, `TransferHandle`, and direct `start_upload/start_download` spawning after command/event tests are green. Update all `test_remote_state`/cleanup/server fixtures. Keep legacy progress type definitions until all frontend compatibility tests pass.

- [ ] **Step 6: Run backend migration verification**

Run: `cargo fmt --all -- --check`

Run: `cargo test -p termlab_remote transfer -- --nocapture`

Run: `cargo test -p termlab_tauri remote::transfer -- --nocapture`

Run: `cargo test --workspace`

Expected: PASS; no `TransferRegistry` or `transfer_progress_tx` references remain; generated TypeScript files match serde names.

- [ ] **Step 7: Commit the backend migration**

```bash
git add crates/termlab_remote crates/termlab_tauri/src crates/termlab_tauri/frontend/types
git commit -m "Wire durable transfer queue commands and events"
```

---

### Task 10: Add the authoritative frontend transfer runtime

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/transfers/data-service.js`
- Create: `crates/termlab_tauri/frontend/app/features/transfers/store.js`
- Create: `crates/termlab_tauri/frontend/app/features/transfers/runtime.js`
- Create: `scripts/tests/test_transfer_store.mjs`
- Modify: `crates/termlab_tauri/frontend/app/core/commands.js`
- Modify: `crates/termlab_tauri/frontend/app/core/tauri-client.js`
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js`
- Modify: `crates/termlab_tauri/frontend/index.html`

**Interfaces:**
- Consumes: Task 9 commands and `transfer-job-updated`/`transfer-queue-summary` events.
- Produces: `window.termlabTransferDataService`, `window.termlabTransferStore`, and singleton `window.termlabTransferRuntime` with `ensureStarted`, `subscribe`, `getSnapshot`, and command methods.
- Tasks 11-13 use only the runtime; they do not listen/invoke independently for queue lifecycle.

- [ ] **Step 1: Write failing pure store tests**

In `test_transfer_store.mjs`, load the real IIFE in a `vm` sandbox. Prove `hydrate(snapshot)` replaces state; a contiguous job update patches one row; a removal deletes it; summary updates counts without replacing jobs; an older event is ignored; a revision gap returns `{needsRefresh: true}` rather than guessing.

```javascript
const store = sandbox.termlabTransferStore.create();
store.hydrate({ revision: 4, jobs: [job('a', 'running')], summary: summary(4) });
assert.deepEqual(store.applyJobEvent({ revision: 5, job: job('a', 'paused') }), { needsRefresh: false });
assert.equal(store.getSnapshot().jobs[0].state.kind, 'paused');
assert.deepEqual(store.applyJobEvent({ revision: 7, job: job('b', 'queued') }), { needsRefresh: true });
```

- [ ] **Step 2: Run the frontend test and verify missing module**

Run: `node scripts/tests/test_transfer_store.mjs`

Expected: FAIL because `features/transfers/store.js` does not exist.

- [ ] **Step 3: Implement the pure store and focused data service**

The data service exposes one function per Task 9 command and constructs camelCase invoke arguments. It contains no DOM/toast/state logic. The store clones snapshots at its public boundary, keeps one authoritative revision, and exposes selectors `activeJobs()` and `historyJobs()` based on terminal state kinds.

- [ ] **Step 4: Write failing runtime ordering/idempotence tests**

Stub `listen` and `invoke`. Prove listeners attach before initial snapshot; events arriving before snapshot are buffered then replayed by revision; a replay gap triggers exactly one snapshot refresh; two `ensureStarted` calls attach one listener pair; every subscriber gets the current snapshot immediately; unsubscribe stops callbacks; command methods delegate once and let backend events update state.

- [ ] **Step 5: Implement the runtime API**

```javascript
const runtime = {
  ensureStarted({ invoke, listen, toast }),
  subscribe(listener),
  getSnapshot(),
  pause(id), resume(id), cancel(id), retry(id), resolve(id, resolution),
  pauseAll(), resumeAll(), reorder(id, before), setPriority(id, priority),
  clearCompleted(), updateSettings(settings), refresh(),
};
```

Buffer job events until the initial `transfer_queue_snapshot` resolves. If revisions are non-contiguous, serialize refreshes behind one in-flight promise. Aggregate terminal or newly-attention-required job updates by `batchId || job.id` for a 300ms window and produce one success/failure/attention summary toast per batch, never one toast per file.

- [ ] **Step 6: Wire script graph, command constants, and main-window startup**

Load data-service→store→runtime before any panel that consumes them. Add new commands/events to both command constant tables. In `tool-window-runtime.init`, call idempotent `ensureStarted({invoke, listen: listenOnCurrentWindow, toast: global.toast})` before built-in registration. Panel-host registration does not initialize app-global shortcuts; Task 11's panel init calls the same idempotent runtime inside that window.

Run: `node scripts/tests/test_transfer_store.mjs`

Run: `node --check crates/termlab_tauri/frontend/app/features/transfers/data-service.js`

Run: `node --check crates/termlab_tauri/frontend/app/features/transfers/store.js`

Run: `node --check crates/termlab_tauri/frontend/app/features/transfers/runtime.js`

Expected: PASS.

- [ ] **Step 7: Commit the frontend boundary before UI work**

```bash
git add crates/termlab_tauri/frontend/app/features/transfers crates/termlab_tauri/frontend/app/core crates/termlab_tauri/frontend/app/tool-window-runtime.js crates/termlab_tauri/frontend/index.html scripts/tests/test_transfer_store.mjs
git commit -m "Add the authoritative frontend transfer runtime"
```

---

### Task 11: Register and render the dense Transfer Center tool window

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/transfers/view.js`
- Create: `crates/termlab_tauri/frontend/app/panels/transfer-center.js`
- Create: `crates/termlab_tauri/frontend/styles/design-system/components/transfer-center.css`
- Create: `scripts/tests/test_transfer_center.mjs`
- Modify: `crates/termlab_tauri/frontend/app/tool-window-runtime.js`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Modify: `scripts/tests/test_panel_host.mjs`

**Interfaces:**
- Consumes: Task 10 runtime snapshot/subscription, existing `toolWindowManager.register` and `utils.formatSize`.
- Produces: `window.termlabTransferCenterView` and `window.transferCenterPanel.init({panelEl, invoke, listen})`.
- Task 12 adds action/dialog behavior through the view's event callbacks without replacing the renderer.

- [ ] **Step 1: Write a failing tool-window registration/mount test**

Extend the real tool-window runtime harness to assert a built-in `transfer-center` registration with title `Transfers`, default zone `bottom`, and one shared `renderFn` that mounts both docked and panel-host contexts. Registration order must leave `file-explorer` as the first/default active bottom window on existing layouts.

- [ ] **Step 2: Add the registration and script/style tags**

Register immediately after `file-explorer`:

```javascript
global.toolWindowManager.register('transfer-center', {
  title: 'Transfers',
  icon: null,
  type: 'built-in',
  defaultZone: 'bottom',
  renderFn: (container) => {
    const panelEl = document.createElement('div');
    panelEl.id = 'transfer-center-panel';
    container.appendChild(panelEl);
    global.transferCenterPanel.init({ panelEl, invoke, listen: listenOnCurrentWindow });
  },
});
```

Do not add a new icon asset; the existing title/strip fallback is sufficient.

- [ ] **Step 3: Write failing renderer tests for Active/History and dense columns**

The DOM harness must assert toolbar controls/counts; columns File/Direction, Host, Destination, Status/Progress, Speed/ETA, Actions; active states excluded from History; Completed/Failed/Cancelled excluded from Active; empty/loading/recovery-error states; one selected row with `aria-selected`; status text present independently of color.

- [ ] **Step 4: Implement view/controller with narrow updates**

`view.js` owns DOM creation/event delegation. `transfer-center.js` owns only selected ID, Active/History tab, runtime subscription, and callbacks. Key rows by `data-job-id`. On progress-only updates, patch the matching progress/status/speed cells and aggregate counts; rebuild the table only for membership/order/filter changes.

Use semantic table/grid roles, buttons with `aria-label`, visible token-based `:focus-visible`, and `aria-live="polite"` only for aggregate summary—not every progress tick.

- [ ] **Step 5: Style the dense bottom/popped layouts with tokens**

Use `--tl-row-h`, `--tl-toolbar-h`, `--tl-panel-bg`, `--tl-border`, `--tl-fg*`, `--tl-row-hover`, `--tl-selection-bg`, `--tl-accent`, and `--tl-danger`; no raw hex. Allow horizontal overflow below narrow widths; never collapse actions over filenames. The same CSS fills a panel host through flex/min-height rules.

- [ ] **Step 6: Run UI and panel-host tests**

Run: `node scripts/tests/test_transfer_center.mjs`

Run: `node scripts/tests/test_panel_host.mjs`

Run: `bash scripts/check_frontend_boundaries.sh .`

Expected: PASS; boundary reports no raw design-system hex or direct Tauri invocation.

- [ ] **Step 7: Commit the read-only Transfer Center slice**

```bash
git add crates/termlab_tauri/frontend/app/features/transfers/view.js crates/termlab_tauri/frontend/app/panels/transfer-center.js crates/termlab_tauri/frontend/styles/design-system/components/transfer-center.css crates/termlab_tauri/frontend/app/tool-window-runtime.js crates/termlab_tauri/frontend/index.html scripts/tests/test_transfer_center.mjs scripts/tests/test_panel_host.mjs
git commit -m "Add the Transfer Center tool window"
```

---

### Task 12: Add Transfer Center actions, dialogs, keyboard flow, and settings

**Files:**
- Create: `crates/termlab_tauri/frontend/app/features/transfers/dialogs.js`
- Modify: `crates/termlab_tauri/frontend/app/features/transfers/view.js`
- Modify: `crates/termlab_tauri/frontend/app/features/transfers/runtime.js`
- Modify: `crates/termlab_tauri/frontend/app/panels/transfer-center.js`
- Modify: `crates/termlab_tauri/frontend/index.html`
- Modify: `scripts/tests/test_transfer_center.mjs`

**Interfaces:**
- Consumes: Task 10 runtime commands, existing `tlDialog`, existing `termlabConnectAuth.run(serverEntryId, startingError, ctx)`, existing files data-service host connect helpers, and generic tool-window shortcut configuration.
- Produces: complete row/toolbar control surface and `window.termlabTransferDialogs`.

- [ ] **Step 1: Write failing action-availability tests**

Assert action sets by state: Running→Pause/Cancel; Paused→Resume/Cancel; Failed→Retry/Details; NeedsConnection→Connect/Cancel; NeedsAttention→Resolve/Cancel; Queued→Pause/priority/reorder/Cancel; terminal→Details only. Resume is absent/disabled unless the reason reports `resumeAvailable: true`.

- [ ] **Step 2: Implement event-delegated row and toolbar actions**

All buttons carry `data-transfer-action` and job ID. The controller maps them to runtime methods and waits only for command acknowledgement; backend events are the sole state update. Pause All/Resume All, clear completed, and Active/History use the same rule.

- [ ] **Step 3: Write failing dialog tests**

Use a stub `tlDialog`. Cover:

- cancel confirmation names the file and calls cancel only on confirm;
- details show endpoint/paths/timestamps/checkpoint/error without secrets;
- destination conflict offers enabled Overwrite/Rename/Skip and Resume only when compatible;
- source changed offers Restart/Skip, never Overwrite;
- Rename requires a non-empty path and sends `{kind: 'rename', destination: value}`;
- concurrency accepts integers 1–32, shows inline validation, and sends both limits together;
- focus returns to the invoking row after close.

- [ ] **Step 4: Implement dialogs using `tlDialog` and safe DOM construction**

Build labels/inputs with `document.createElement`/`textContent`; never interpolate paths/errors into `innerHTML`. Clear input values on close. Destructive Cancel and Overwrite get explicit copy and button treatment; Escape dismisses without mutation.

- [ ] **Step 5: Write failing keyboard tests and implement root-scoped behavior**

When focus is within the panel: ArrowUp/ArrowDown select rows, Space pauses/resumes the selected eligible job, Enter opens Resolve or Details, Delete opens Cancel confirmation. Inputs/buttons retain native key behavior. Attach `keydown` to the panel root, not `document` and not the app keyboard router, so panel hosts retain their zero-global-shortcut invariant.

The configurable focus shortcut needs no new core setting: `tool_window_shortcuts['transfer-center']` is automatically exposed by the generic settings/tool-window runtime after registration. Add a test that the registered ID appears in the existing tool-window shortcut inventory.

- [ ] **Step 6: Implement explicit configured-host reconnect**

For `TransferEndpoint::Configured`, Connect first calls existing `sftp_connect_host`; typed auth failures enter `termlabConnectAuth.run` with the real starting error and files data-service dependencies. On a won connection, call `runtime.resume(job.id)`. For `AdHoc`, show actionable copy telling the user to reconnect a matching `user@host:port` session, then use Resume; do not invent/store credentials or auto-connect.

- [ ] **Step 7: Run complete interaction tests and commit**

Run: `node scripts/tests/test_transfer_center.mjs`

Run: `node --check crates/termlab_tauri/frontend/app/features/transfers/dialogs.js`

Run: `bash scripts/check_frontend_boundaries.sh .`

Expected: PASS.

```bash
git add crates/termlab_tauri/frontend/app/features/transfers crates/termlab_tauri/frontend/app/panels/transfer-center.js crates/termlab_tauri/frontend/index.html scripts/tests/test_transfer_center.mjs
git commit -m "Add Transfer Center controls and conflict resolution"
```

---

### Task 13: Migrate files-panel/editor producers and compatibility feedback

**Files:**
- Modify: `crates/termlab_tauri/frontend/app/features/files/data-service.js`
- Modify: `crates/termlab_tauri/frontend/app/features/files/transfers.js`
- Modify: `crates/termlab_tauri/frontend/app/panels/files-panel.js`
- Modify: `crates/termlab_tauri/frontend/app/features/editor/editor-service.js`
- Modify: `scripts/tests/test_files_transfers.mjs`
- Modify: `scripts/tests/test_editor_remote_transfer.mjs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/runner.rs`

**Interfaces:**
- Consumes: Task 9 optional enqueue origin/conflict fields, new queue events, and legacy progress events.
- Produces: FilesPanel jobs with Normal priority/Ask conflicts; Editor jobs with Interactive priority/explicit Overwrite; pane badges/refresh and editor promises remain behavior-compatible.

- [ ] **Step 1: Write failing producer-payload tests**

Files data service must invoke with `origin: 'filesPanel'` and conflict `{kind: 'ask'}`. Editor remote open/save must invoke with `origin: 'editor'` and conflict `{kind: 'overwrite'}` so opening its private temp path and intentionally saving the already-open remote file do not stall on ordinary existence.

```javascript
assert.deepEqual(upload.args, {
  paneId: 4,
  localPath: '/tmp/edit.txt',
  remotePath: '/srv/edit.txt',
  origin: 'editor',
  conflictPolicy: { kind: 'overwrite' },
});
```

- [ ] **Step 2: Implement origin/conflict arguments without duplicating command wrappers**

Extend existing files data-service transfer functions with an options object and have all files-panel call sites pass FilesPanel/Ask. Update the two editor invoke call sites only; keep its listener-before-start/early-event/stall-guard logic until the legacy event compatibility period ends.

- [ ] **Step 3: Replace per-file progress toasts with queue-aware badges and aggregate runtime toasts**

`features/files/transfers.js` retains pane `transferStatus` updates from legacy byte progress. Remove its private `.fp-progress-toast` creation/completion toasts; Task 10 runtime owns aggregate batch toasts. Add `handleTransferSnapshot` so NeedsAttention and NeedsConnection display a non-spinning attention badge, terminal jobs clear badges, and newly Completed jobs refresh both panes only after commit/handle close. `files-panel.js` subscribes through `termlabTransferRuntime.subscribe` and never adds a second raw queue-event listener.

- [ ] **Step 4: Add freshness regression tests at the real runner boundary**

In Rust runner tests, use a temp local source and fake remote `TransferIo`: run one upload, rewrite the same path, enqueue/run a new job, and assert the second sink bytes are the new contents/fingerprint. Mirror for download with fake remote contents changed between jobs. Assert no file/source handle is retained by the fake after a run. Add paused-source-change test: bytes/checkpoint from version A plus current version B returns `NeedsAttention(SourceChanged)` until Restart clears the attempt.

- [ ] **Step 5: Run producer/feedback regression suites**

Run: `node scripts/tests/test_files_transfers.mjs`

Run: `node scripts/tests/test_editor_remote_transfer.mjs`

Run: `cargo test -p termlab_tauri remote::transfer_queue::runner::tests -- --nocapture`

Expected: PASS; existing editor early-completion, failure, cancellation, and stall cases still settle exactly once.

- [ ] **Step 6: Commit migrated producers**

```bash
git add crates/termlab_tauri/frontend/app/features/files crates/termlab_tauri/frontend/app/panels/files-panel.js crates/termlab_tauri/frontend/app/features/editor/editor-service.js scripts/tests/test_files_transfers.mjs scripts/tests/test_editor_remote_transfer.mjs crates/termlab_tauri/src/remote/transfer_queue/runner.rs
git commit -m "Route files and editor transfers through the durable queue"
```

---

### Task 14: Harden recovery, add live verification, reconcile documentation, and run the full gate

**Files:**
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/store.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/engine.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/artifacts.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/runner.rs`
- Create: `docs/superpowers/notes/sftp-transfer-queue-manual-checklist.md`
- Modify: `docs/superpowers/specs/2026-08-23-sftp-durable-transfer-queue-design.md`

**Interfaces:**
- Consumes: the complete queue/backend/frontend implementation.
- Produces: verified startup/commit recovery, opt-in live SFTP coverage, manual evidence, and an implementation-status spec update.

- [ ] **Step 1: Add restart/recovery integration tests across modules**

Use temp stores/artifacts to cover: crash after each commit phase; display progress ahead of durable checkpoint truncates back; missing partial; partial shorter than checkpoint lowers/persists checkpoint before copying; retry deadline restored but not dispatched while suspended; corrupt store quarantined and visible in snapshot; disk-full/permission error preserves artifacts; cancel cleanup failure records the exact leftover path.

- [ ] **Step 2: Add an ignored live OpenSSH/SFTP test using environment credentials**

Keep it in `runner.rs`'s `#[cfg(test)]` module so it can reach crate-private APIs. Require `TERMLAB_TEST_SFTP_HOST`, `TERMLAB_TEST_SFTP_PORT`, `TERMLAB_TEST_SFTP_USER`, and `TERMLAB_TEST_SFTP_KEY`; return early with a clear skip message when absent. Mark `#[ignore = "requires an explicitly configured disposable OpenSSH server"]`. Build a `ServerEntry`/key `SshCredentials`, connect with a no-prompt test `RemoteCallbacks`, register the resulting handle in a test `RemoteState`, and run the ordinary `SftpTransferJobRunner`. In a UUID-named disposable remote directory, verify upload/download, pause/resume offsets, remote mtime handling, overwrite backup/promotion, cleanup, and second-transfer freshness; clean only that exact UUID directory.

Run when configured:

```bash
cargo test -p termlab_tauri live_sftp_queue_roundtrip -- --ignored --nocapture
```

- [ ] **Step 3: Create the tracked manual/reconciliation checklist**

Document exact evidence fields (date/platform/server/OpenSSH version/commit) and checks for:

- local source changed after app open then uploaded twice;
- remote source changed after app open then downloaded twice;
- both directions pause→quit→relaunch→no network→explicit reconnect→resume;
- changed source while paused requires Restart;
- Overwrite/Rename/Skip and compatible Resume while unrelated jobs continue;
- 3 global/2 host limits across two windows and destination serialization;
- interruption at every commit phase preserves an intact old/new file;
- Transfer Center dock/resize/hide/pop-out, keyboard/focus, density, errors, Active/History;
- persisted JSON/log inspection for credentials;
- gap reconciliation: mark durable queue, pause/resume, concurrency, retry, history, conflicts, and Transfer Center as implemented; list recursive/DnD/sync/advanced browsing/resilience/other protocols as later projects.

Do not edit or stage the user's untracked `SFTP_FILEZILLA_GAP.md`; the note contains the exact verified mapping for the owner to apply.

- [ ] **Step 4: Update design status only after evidence is green**

Change the spec status from `Approved` to `Implemented` and add the implementation branch/commit plus the manual checklist link. Do not rewrite settled design decisions to disguise implementation deviations; record any accepted deviation explicitly.

- [ ] **Step 5: Run formatting, focused tests, full workspace, and frontend gates**

Run: `cargo fmt --all -- --check`

Run: `cargo test -p termlab_remote transfer -- --nocapture`

Run: `cargo test -p termlab_tauri remote::transfer -- --nocapture`

Run: `cargo test --workspace`

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Run every frontend VM suite:

```bash
for test_file in scripts/tests/*.mjs; do node "$test_file"; done
```

Run: `python3 scripts/tests/test_extract_tokens.py`

Run: `bash scripts/check_frontend_boundaries.sh .`

Run: `git diff --check`

Expected: every command exits 0. If a platform/live test cannot run, record the exact reason and do not claim that verification.

- [ ] **Step 6: Run secret/state ownership sweeps**

Run:

```bash
rg -n "password|passphrase|private_key|ssh_handle|session_handle" crates/termlab_tauri/src/remote/transfer_queue crates/termlab_tauri/frontend/app/features/transfers
```

Inspect every hit; only explicit negative tests/documentation or runtime-only connection code may remain. Also verify:

```bash
rg -n "TransferRegistry|transfer_progress_tx|start_upload\(|start_download\(" crates/termlab_tauri crates/termlab_remote
```

Expected: no obsolete active-only registry/start path remains.

- [ ] **Step 7: Commit documentation/hardening and push**

```bash
git add crates/termlab_remote crates/termlab_tauri docs/superpowers/specs/2026-08-23-sftp-durable-transfer-queue-design.md docs/superpowers/notes/sftp-transfer-queue-manual-checklist.md
git commit -m "Harden SFTP queue recovery and document verification"
git push -u origin HEAD
```

Do not open a pull request unless the user explicitly asks.
