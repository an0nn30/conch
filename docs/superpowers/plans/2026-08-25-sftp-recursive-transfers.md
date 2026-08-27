# Recursive SFTP Transfers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Folder transfers through the durable queue (both directions) with per-batch roll-up progress in the Transfer Center, plus drag-and-drop between panes and from the OS.

**Architecture:** A backend expansion task walks the tree through a lister trait (local FS / SFTP fakes for tests), creates destination directories, and incrementally enqueues ordinary batch-tagged jobs — nothing in the reducer/scheduler/runner changes. A small additive `BatchInfo` record persists per batch; aggregates are derived at event time and ride the existing snapshot/delta events as a `batches` projection. The Transfer Center groups member rows under derived header rows in both layouts. DnD feeds the same two enqueue commands.

**Tech Stack:** Rust (tokio, existing termlab_remote sftp helpers), Tauri v2 events, vanilla JS frontend, node test scripts.

**Spec:** `docs/superpowers/specs/2026-08-25-sftp-recursive-transfers-design.md`

## Global Constraints

- Work in `/Users/dustin/projects/conch/.worktrees/sftp-durable-transfer-queue-impl` on branch `feat/sftp-recursive-transfers`. Before ANY commit, `git rev-parse --abbrev-ref HEAD` must print exactly that branch; otherwise stop. Never touch `/Users/dustin/projects/conch`. Never push (controller pushes).
- Persisted schema stays v1: `BatchInfo` map is additive with `#[serde(default)]`; older stores must load (test-proven). No version bump.
- Members are ordinary jobs: zero changes to `reducer.rs`, `scheduler.rs`, `runner.rs`.
- Expansion never authenticates and never consumes a scheduler slot; symlinks are skipped and recorded; hidden files are included; empty directories are created; created directories are never removed on cancel.
- Batch aggregates are derived at emission time, never persisted (only `BatchInfo`'s identity/expansion/discovered fields persist).
- Restart maps `Running` expansion → `Interrupted { reason: "app closed during expansion" }`.
- Serde camelCase everywhere; TS types regenerate via `cargo test -p termlab_tauri` (ts-rs).
- TDD per task; short imperative commits, no Co-Authored-By. Rust: `cargo test -p termlab_tauri` (and `-p termlab_remote` when touched). Frontend: the named `.mjs` suites per task plus the full `scripts/tests` sweep in the final task.

## File Structure

- Create: `crates/termlab_tauri/src/remote/transfer_queue/batch.rs` — `BatchInfo`, expansion states, aggregate derivation (pure).
- Create: `crates/termlab_tauri/src/remote/transfer_queue/expansion.rs` — path mapping (pure) + the walker over a lister trait + directory creation + incremental enqueue.
- Modify: `transfer_queue/{model.rs, store.rs, events.rs, engine.rs, mod.rs}` — persistence, projections, commands, restart marking.
- Modify: `crates/termlab_tauri/src/remote/transfer_commands.rs` — the two new Tauri commands; `crates/termlab_tauri/src/lib.rs` — command registration.
- Modify: frontend `features/transfers/{store.js, view.js, data-service.js}`, `panels/transfer-center.js` (cancel-batch dialog hook), `features/files/…` + `panels/files-panel.js` (context menu, DnD), `styles/design-system/components/transfer-center.css`, `styles/panels.css` (drop highlight), generated `frontend/types/*`.
- Tests: module `#[cfg(test)]` blocks; `scripts/tests/test_transfer_center.mjs`, `test_transfer_store.mjs`, new `scripts/tests/test_files_dnd.mjs`.

---

### Task 1: Batch model, persistence, and aggregate derivation

**Files:**
- Create: `crates/termlab_tauri/src/remote/transfer_queue/batch.rs`
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/mod.rs` (declare `pub(crate) mod batch;`), `model.rs` (document + snapshot fields), `store.rs` (persistence pass-through)

**Interfaces:**
- Consumes: `TransferJob` (existing: `batch_id: Option<Uuid>`, `state.kind`, `bytes_transferred`, `total_bytes`, `speed_bytes_per_second`, `direction`).
- Produces (Tasks 2–5 rely on these exact names):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
#[ts(export)]
pub enum BatchExpansion {
    Running,
    Complete,
    Interrupted { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BatchInfo {
    pub id: Uuid,
    pub name: String,
    pub direction: TransferDirection,
    pub expansion: BatchExpansion,
    #[ts(as = "f64")]
    pub discovered_files: u64,
    #[ts(as = "f64")]
    pub discovered_bytes: u64,
    /// Symlinks (and other skipped entries) recorded during expansion.
    #[serde(default)]
    pub skipped: Vec<String>,
    #[ts(as = "f64")]
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BatchAggregate {
    pub info: BatchInfo,
    #[ts(as = "f64")]
    pub files_done: u64,
    #[ts(as = "f64")]
    pub bytes_done: u64,
    #[ts(as = "Option<f64>")]
    pub speed_bytes_per_second: Option<u64>,
    #[ts(as = "Option<f64>")]
    pub eta_seconds: Option<u64>,
}

pub fn derive_batch_aggregates(
    batches: &BTreeMap<Uuid, BatchInfo>,
    jobs: &[TransferJob],
) -> Vec<BatchAggregate>; // ordered by info.created_at_ms then id

pub fn compact_batches(
    batches: &mut BTreeMap<Uuid, BatchInfo>,
    jobs: &[TransferJob],
); // drop batches with zero remaining member jobs
```

- `TransferQueueDocument` gains `#[serde(default)] pub batches: BTreeMap<Uuid, BatchInfo>`; `TransferQueueSnapshot` gains `pub batches: Vec<BatchAggregate>`.

- [ ] **Step 1: Write failing tests** in `batch.rs`'s `#[cfg(test)]` module. Build job fixtures with the module's own helper (copy the fixture-builder shape from `events.rs`'s tests — a minimal `TransferJob` literal). Cases, with exact expectations:
  - `derive_sums_done_files_bytes_and_speed`: 3 members of one batch — completed (bytes 100/100), running (bytes_transferred 40, total 100, speed 50), queued (0/200, speed 0 = unknown sentinel) → `files_done 1`, `bytes_done 140`, `speed Some(50)`, `eta Some((discovered_bytes 400 - 140) / 50 = 5)` given `discovered_bytes: 400`, `discovered_files: 3`.
  - `derive_speed_none_when_no_active_member` → speed None, eta None.
  - `derive_ignores_jobs_of_other_batches_and_batchless`.
  - `derive_orders_by_created_then_id` (two batches).
  - `compact_drops_only_fully_cleared_batches` (batch with one remaining history job survives; batch with none is dropped).
  - `document_without_batches_field_deserializes_empty` — feed the v1 JSON document literal (mirror the existing back-compat test in `store.rs`) without a `batches` key.
- [ ] **Step 2: Run** `cargo test -p termlab_tauri batch` → compile failure (module missing).
- [ ] **Step 3: Implement.** Derivation rules: `files_done` counts members with `state.kind == completed` (skip-completions count — they are `completed` with skip metadata); `bytes_done` sums `bytes_transferred` clamped to each member's `total_bytes` when known; speed sums `speed_bytes_per_second` over members in `running` state, treating the persisted `0` sentinel as unknown (see the existing sentinel note in `model.rs`/frontend store) → `None` if the sum is 0; `eta = (discovered_bytes.saturating_sub(bytes_done)) / speed` when speed is Some and non-zero. Wire the two model fields; store persistence is automatic via serde (confirm `store.rs` round-trips the document type without field lists — if it enumerates fields anywhere, extend).
- [ ] **Step 4: Run** `cargo test -p termlab_tauri` fully green; confirm `frontend/types/BatchInfo.ts`/`BatchAggregate.ts`/`BatchExpansion.ts` generated.
- [ ] **Step 5: Commit** `Add batch model with derived roll-up aggregates`.

---

### Task 2: Batches ride the queue events end-to-end

**Files:**
- Modify: `crates/termlab_tauri/src/remote/transfer_queue/events.rs` (delta payload + emission), `engine.rs` (snapshot construction, compaction on clear-completed), frontend `features/transfers/store.js`, `scripts/tests/test_transfer_store.mjs`

**Interfaces:**
- Consumes: Task 1's types/functions.
- Produces: the job-delta payload and snapshot both carry `batches: Vec<BatchAggregate>` (full projection each emission — batch count is small; no per-batch deltas). Frontend store snapshot exposes `snapshot.batches` (array, cloned on egress like `jobs`).

- [ ] **Step 1 (Rust, RED):** in `events.rs` tests, extend the existing exact-payload test (`job_event_payload_is_a_complete_atomic_delta`) fixture with one `BatchInfo` + a member job and assert the serialized payload contains the full `batches` array with derived numbers (exact JSON, matching that test's style). Add `snapshot_includes_batch_aggregates` in `engine.rs` tests mirroring its nearest snapshot test.
- [ ] **Step 2: Run** → red for the right reason (missing field).
- [ ] **Step 3: Implement:** emission sites call `derive_batch_aggregates(&document.batches, &document.jobs)`; `clear_completed` calls `compact_batches` after removing jobs. Keep coalescing rules untouched.
- [ ] **Step 4 (frontend, RED then GREEN):** in `test_transfer_store.mjs`, extend the snapshot/delta fixtures with a `batches` array and assert `store.getSnapshot().batches` hydrates, replaces wholesale on delta, and is defensively cloned (mutate the exposed array; re-read). Then implement in `store.js` following exactly how `jobs` ingress/egress works (including the revision rules — batches accompany whichever payload carries them; a payload without `batches` keeps the previous projection).
- [ ] **Step 5: Run** `cargo test -p termlab_tauri` + `node scripts/tests/test_transfer_store.mjs` green.
- [ ] **Step 6: Commit** `Project batch aggregates through queue events`.

---

### Task 3: Path mapping and the expansion walker

**Files:**
- Create: `crates/termlab_tauri/src/remote/transfer_queue/expansion.rs`
- Modify: `transfer_queue/mod.rs` (declare module)

**Interfaces:**
- Consumes: nothing outside std/tokio (the walker is generic).
- Produces:

```rust
/// Map a discovered source entry onto its destination path.
/// POSIX remote paths use '/'; local paths use the platform separator.
pub fn map_destination(
    source_root: &str,
    dest_root: &str,
    entry_path: &str,
    remote_dest: bool, // true => join with '/', false => platform join
) -> Result<String, String>; // Err if entry_path is not under source_root

#[derive(Debug, Clone, PartialEq)]
pub enum WalkEntry {
    Dir { path: String },
    File { path: String, size: u64 },
    SkippedSymlink { path: String },
}

#[async_trait]
pub trait TreeLister: Send + Sync {
    /// Immediate children of `path`, each tagged dir/file/symlink with size.
    async fn list(&self, path: &str) -> Result<Vec<WalkEntry>, String>;
}

/// Breadth-first walk: emits the root's children downward, parents before
/// children, yielding between directories. Calls `on_entry` for every entry
/// (including empty dirs and skipped symlinks). Stops early if `cancelled()`.
pub async fn walk_tree<F, Fut, C>(
    lister: &dyn TreeLister,
    root: &str,
    mut on_entry: F,
    cancelled: C,
) -> Result<(), String>
where
    F: FnMut(WalkEntry) -> Fut,
    Fut: Future<Output = Result<(), String>>,
    C: Fn() -> bool;
```

Task 4 implements `TreeLister` for the local FS and SFTP and drives enqueue from `on_entry`.

- [ ] **Step 1 (RED):** tests with an in-memory fake lister (BTreeMap path → children):
  - `maps_nested_paths_both_separators`: `map_destination("/a/b", "/home/x", "/a/b/c/d.txt", true)` == `"/home/x/c/d.txt"`; same with `remote_dest false` producing a platform-joined path; root itself maps to dest_root; entry outside root errors; unicode segment preserved; trailing-slash roots normalized.
  - `walk_emits_parents_before_children_breadth_first` (fixture tree 3 levels; assert exact emission order).
  - `walk_includes_empty_dirs_and_hidden_files`.
  - `walk_skips_symlinks_with_marker` (SkippedSymlink emitted, not descended).
  - `walk_stops_when_cancelled` (cancel after N entries; no further lister calls — count them).
  - `walk_surfaces_lister_error_with_path` (subdir list error → Err containing the path; prior entries already emitted).
- [ ] **Step 2: Run** → module missing.
- [ ] **Step 3: Implement** (BFS queue of dirs; emit `Dir` when dequeued, then its files/symlinks, enqueue subdirs; `tokio::task::yield_now().await` per directory).
- [ ] **Step 4: Run** green; full `cargo test -p termlab_tauri` green.
- [ ] **Step 5: Commit** `Add tree walker and destination path mapping`.

---

### Task 4: Expansion task, listers, and the two commands

**Files:**
- Modify: `expansion.rs` (listers + `run_expansion`), `engine.rs` (batch bookkeeping commands: create/update BatchInfo, cancel-batch, restart marking), `transfer_commands.rs` (Tauri commands), `crates/termlab_tauri/src/lib.rs` (register commands)

**Interfaces:**
- Consumes: `walk_tree`/`map_destination`/`TreeLister` (Task 3); `BatchInfo`/`BatchExpansion` (Task 1); `build_transfer_request` (`transfer_commands.rs:120`) and `queue.enqueue(NewTransferJob)` (`engine.rs:58`); SFTP session resolution + `list_dir` per `sftp_commands.rs:58-68`; `termlab_remote::sftp::mkdir` (`sftp.rs:163`), `tokio::fs::create_dir_all`; the local lister mirrors `local_fs::list_dir` semantics (`sftp_commands.rs:195`).
- Produces:
  - Tauri command `transfer_enqueue_recursive(window, remote, queue, pane_id: u32, direction: TransferDirectionCommand, source_path: String, dest_path: String) -> Result<String /* batch id */, String>` — validates source is a directory, creates the batch (engine call), spawns the expansion task, returns immediately with the batch id.
  - Tauri command `transfer_cancel_batch(queue, batch_id: String) -> Result<(), String>`.
  - Engine handle methods (exact names for Task 5's data-service): `create_batch(info) -> Result<(), String>`, `update_batch(id, expansion, discovered_files, discovered_bytes, skipped) -> Result<(), String>`, `cancel_batch(id) -> Result<(), String>` — cancel iterates non-terminal members through the existing per-job cancel path and flips a shared cancellation flag the expansion task polls via `walk_tree`'s `cancelled()`.
  - Restart: wherever the engine loads the document at startup (find the recovery mapping that pauses running jobs), map `BatchExpansion::Running` → `Interrupted { reason: "app closed during expansion".into() }`.
  - Expansion behavior per spec: `Dir` entries create the mapped destination directory (upload: `mkdir` via the session, already-exists tolerated — match on the error the same way `sftp_commands.rs`'s mkdir caller does, or stat-first; download: `create_dir_all`); `File` entries build the request via `build_transfer_request` with `batch_id: Some(batch)` (extend that builder with a `batch_id: Option<Uuid>` parameter — the two existing callers pass `None`); `SkippedSymlink` appends to `BatchInfo.skipped`; totals update via `update_batch` coalesced (every 25 entries and at completion); walk error → `Interrupted { reason }`.
- [ ] **Step 1 (RED):** engine-level integration tests (mirror the nearest engine test harness with its fake runner):
  - `recursive_expansion_enqueues_batch_members_and_completes`: fake lister tree (2 dirs, 3 files, 1 empty dir, 1 symlink) driven through `run_expansion` with a recording directory-creator and the real engine handle → every file enqueued with the batch id, dirs created parents-first including the empty one, symlink recorded in `skipped`, final `BatchInfo` `Complete` with files 3 / bytes summed.
  - `expansion_error_interrupts_batch_but_keeps_members`.
  - `cancel_batch_cancels_members_and_stops_expansion` (lister call count stops growing; members reach cancelled via existing path).
  - `restart_marks_running_expansion_interrupted` (document with `Running` batch loaded → `Interrupted`, members untouched) — place beside the existing restore tests.
  - Command-layer: `transfer_cancel_batch` and `transfer_enqueue_recursive` argument plumbing tests only if the existing command tests have a harness for it; otherwise cover via the engine methods and note it.
- [ ] **Step 2: Run** → red (methods missing).
- [ ] **Step 3: Implement.** `run_expansion(lister, dir_creator, engine_handle, batch, source_root, dest_root, remote_dest, request_builder)` lives in `expansion.rs`, generic over closures so the tests above need no Tauri context. The Tauri command wires the real listers: upload → local FS lister + remote mkdir creator over the pane's session; download → SFTP lister + `create_dir_all`. Session/pane resolution copies `sftp_list_dir`'s pattern exactly.
- [ ] **Step 4: Run** `cargo test -p termlab_tauri` fully green.
- [ ] **Step 5: Commit** `Add recursive expansion task and batch commands`.

---

### Task 5: Transfer Center batch grouping

**Files:**
- Modify: `frontend/app/features/transfers/view.js`, `panels/transfer-center.js` (route the new header action), `features/transfers/data-service.js` (`cancelBatch`), `features/transfers/runtime.js` (expose `cancelBatch`), `styles/design-system/components/transfer-center.css`, `scripts/tests/test_transfer_center.mjs`

**Interfaces:**
- Consumes: `snapshot.batches` (Task 2's frontend projection: `[{ info: { id, name, direction, expansion: {kind, reason?}, discoveredFiles, discoveredBytes, skipped, createdAtMs }, filesDone, bytesDone, speedBytesPerSecond, etaSeconds }]`), invoke command `transfer_cancel_batch`.
- Produces: header rows `tr.tl-transfer-center__batch` with `data-batch-id`, containing name+direction, `filesDone/filesTotal` (append `+` while `expansion.kind === 'running'`, append an `⚠ expansion interrupted` marker with the reason as title when `interrupted`), `formatSize(bytesDone) of formatSize(discoveredBytes)`, speed, ETA, and a cancel button `data-transfer-action="cancel-batch"`. Members sort under their header (batch groups ordered as delivered; batchless rows keep today's order after/between groups — simplest: render batch groups first in delivered order, then batchless rows, both stable).
- [ ] **Step 1 (RED):** extend `test_transfer_center.mjs`: snapshot fixture with one batch (running expansion) + two member jobs + one batchless job → assert header row exists with the exact text pieces (`3/5+`, byte strings via the harness `formatSize`), members render beneath it, batchless row unaffected; interrupted fixture shows the marker; clicking cancel-batch fires the confirm dialog then `runtime.cancelBatch('...')` (mirror the existing cancel-row dialog test); keyboard selection still walks member rows only. Card-layout concerns are CSS-only — assert only DOM/classes here.
- [ ] **Step 2: Run** → red (no batches handling).
- [ ] **Step 3: Implement** view grouping (derive groups in `render` from `snapshot.batches` + member partition; patch-friendly: reuse the existing row-record map for members, rebuild header rows on batch changes), data-service + runtime `cancelBatch(id)` passthrough, dialog via the existing `showCancel` pattern with batch copy, CSS: table grouping row styles + `@container` card-mode header card (follow the existing narrow block).
- [ ] **Step 4: Run** the suite + full sweep of transfer tests green.
- [ ] **Step 5: Commit** `Group Transfer Center rows under batch headers`.

---

### Task 6: Files-panel folder context menu

**Files:**
- Modify: `frontend/app/panels/files-panel.js` (row menu builder ~line 1253 area), `features/files/data-service.js` (`transferRecursive`), `scripts/tests/test_files_transfers.mjs`

**Interfaces:**
- Consumes: invoke command `transfer_enqueue_recursive` with `{ paneId, direction, sourcePath, destPath }` (camelCase per Tauri arg conventions used by `transferUpload`).
- Produces: on directory rows only, a menu item labeled `Upload Folder` (local pane) / `Download Folder` (remote pane) calling `filesDataService.transferRecursive(invoke, paneId, direction, entryPath, oppositePaneCurrentPath)`; file rows unchanged.
- [ ] **Step 1 (RED):** in `test_files_transfers.mjs` (which already drives the data-service), add `transferRecursive` invoking `transfer_enqueue_recursive` with the exact arg object. Menu wiring: assert via the files-panel harness only if that test already mounts the menu (read it first); otherwise data-service coverage + a note.
- [ ] **Step 2–4:** standard RED→implement→GREEN (`node scripts/tests/test_files_transfers.mjs`).
- [ ] **Step 5: Commit** `Add folder transfer context menu items`.

---

### Task 7: Drag-and-drop between panes

**Files:**
- Modify: `frontend/app/features/files/pane-view.js` (draggable rows + drop target wiring), `panels/files-panel.js` (routing deps), `styles/panels.css` (`.fp-pane.is-drop-target` highlight via theme tokens)
- Create: `scripts/tests/test_files_dnd.mjs`

**Interfaces:**
- Consumes: `d.onDropEntries(payload)` dep provided by files-panel; drag payload serialized into `dataTransfer` under MIME `application/x-termlab-entry` as JSON `{ paneKind: 'local'|'remote', paneId, path, isDir }`.
- Produces: rows get `draggable="true"` + `dragstart` handler writing the payload; each pane root handles `dragover` (accept only when payload pane-kind differs; `preventDefault` + `is-drop-target` class) and `drop` (parse payload → files-panel routes: `isDir` → `transferRecursive`, file → existing single-file transfer, destination = target pane's `currentPath`); `dragleave`/`drop` clear the class. Same-kind payloads and foreign drags fall through untouched (the OS-drop path is Task 8's).
- [ ] **Step 1 (RED):** `test_files_dnd.mjs` drives `renderPane` in a VM with recording elements that support the drag event shapes (extend the recording-element pattern from `test_pane_toolbar_layout.mjs` with `dataTransfer` fakes): dragstart writes the exact payload; dragover from opposite kind prevents default and sets the class; same-kind does neither; drop parses and calls `d.onDropEntries` with `{ source: payload, targetPaneKind, targetPath }`; class cleared after drop/leave.
- [ ] **Step 2–4:** RED → implement (wiring in `renderPane` follows the existing guarded-query style; routing in files-panel maps `onDropEntries` to the two transfer calls with toasts on error) → GREEN + existing pane tests still green.
- [ ] **Step 5: Commit** `Support dragging entries between the file panes`.

---

### Task 8: OS file drops onto the remote pane

**Files:**
- Modify: `frontend/app/panels/files-panel.js` (native drop listener + hit-test), `features/files/data-service.js` if a stat helper is needed (`local_list_dir`/existing `localStat`-equivalent — check what exists to distinguish file vs dir for a dropped path; `sftp_commands.rs:195 local_list_dir` on the parent or an existing `local_stat` command — grep first), plus `test_files_dnd.mjs` additions

**Interfaces:**
- Consumes: the app's Tauri event bridge (grep `frontend/app/core/tauri-client.js` for the `listen` wrapper; Tauri v2 emits `tauri://drag-drop` with `{ paths: string[], position: {x, y} }`, plus `tauri://drag-enter/over/leave`). The webview's default drag-drop handling must remain enabled (verify `tauri.conf.json` doesn't set `dragDropEnabled: false`).
- Produces: files-panel subscribes once in `init` (main-window listen, same `opts.listen` it already uses for `transfer-progress`): on drop, hit-test `position` against the remote pane root's `getBoundingClientRect()`; misses are ignored; without an active remote session show the existing "not connected" toast; hits route each path — directory → `transferRecursive` upload, file → `transferUpload` — into the remote pane's `currentPath`. Enter/over events toggle the same `is-drop-target` class on the remote pane; leave clears it.
- [ ] **Step 1 (RED):** extend `test_files_dnd.mjs`: a `handleNativeDrop({ paths, position }, deps)` pure-ish handler exported from files-panel (or a small extracted module if files-panel's VM load is impractical — prefer extracting `features/files/native-drop.js` with `resolveNativeDrop(position, paneRect, sessionActive) -> 'ignore' | 'no-session' | 'accept'` + routing; test that): hit/miss rects, no-session path, dir-vs-file routing given a stat callback.
- [ ] **Step 2–4:** RED → implement → GREEN; manual note in report that a real Finder drop is verified in Task 9's checklist (JSDOM can't produce native drops).
- [ ] **Step 5: Commit** `Upload OS file drops through the remote pane`.

---

### Task 9: Live tree round-trip, checklist, docs, full gate

**Files:**
- Modify: the live-gated test module (same harness as `live_pipelined_*`), `docs/superpowers/notes/sftp-transfer-queue-manual-checklist.md`, `docs/superpowers/specs/2026-08-25-sftp-recursive-transfers-design.md` (status)

- [ ] **Step 1:** add `live_recursive_roundtrip_preserves_tree`: build a local fixture tree (nested dirs, one empty dir, unicode filename, a hidden dotfile), run the expansion upload against the live env into a UUID remote dir, then recursive download into a second temp dir, then compare trees (walk both; same relative paths, same bytes). Skips cleanly without env (existing convention).
- [ ] **Step 2:** manual checklist: add a "Recursive transfers & drag-and-drop" section with rows: Finder-drop folder onto remote pane; pane-to-pane drags both directions; cancel a large batch mid-flight; kill app mid-expansion → relaunch shows `Interrupted` batch with intact members. All pending-evidence.
- [ ] **Step 3:** spec status → Implemented with branch/commit-range note (mirror the pipelined spec's convention, including the live-test disclosure appropriate to what actually ran).
- [ ] **Step 4: Full gate:** `cargo test --workspace` green; full `scripts/tests` sweep zero failures; `bash scripts/check_frontend_boundaries.sh` clean except the documented `tl-dialog.js:334` exception.
- [ ] **Step 5: Commit** `Verify recursive transfers and reconcile docs`.

---

## Self-Review

- **Spec coverage:** rules 1–2 → Tasks 3–4; rule 3 (no reducer/scheduler/runner changes) → Global Constraints + Task 4 building on plain enqueue; rules 5/9/10 → Task 3 tests + Task 4 dir creation; rules 6–7 → Tasks 1–2 + restart marking in Task 4; rule 8 → Tasks 7–8 routing through the two commands only; batch UI → Task 5; context menu → Task 6; failure table → Tasks 3/4 error tests; live round-trip + checklist → Task 9. Batch cancel → Tasks 4 (backend) and 5 (UI).
- **Placeholder scan:** wiring-heavy steps (5–8) anchor to named existing patterns and tests rather than inline code — each names the exact files, behaviors, payload shapes, and assertions; no TBDs. Task 8 pre-authorizes the extract-a-module fallback so the executor isn't guessing.
- **Type consistency:** `BatchInfo`/`BatchAggregate`/`BatchExpansion` defined once (Task 1) and consumed by exact field names in Tasks 2/5; command names and arg shapes match between Tasks 4/6/8; the DnD payload MIME/shape matches between Tasks 7 producer and consumer.
