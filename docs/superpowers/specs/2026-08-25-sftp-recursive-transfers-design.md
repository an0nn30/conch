# Recursive SFTP Transfers, Batch Progress, and Drag-and-Drop — Design

**Status:** Approved
**Date:** 2026-08-25
**Scope:** Transfer whole folders through the durable SFTP queue (upload and
download), report roll-up progress per batch (files, bytes, aggregate speed,
ETA), and start transfers by drag-and-drop — between the two panes and from
the OS file manager into the remote pane. This is cluster 2 of the transfer
roadmap in the durable-queue design, minus multi-select, which is deferred.

## Product rules

1. **The backend owns expansion.** Tree enumeration, directory creation, and
   batch job creation happen in one backend task per request
   (`transfer_enqueue_recursive`). The frontend sends a single command naming
   the source, destination, and direction; it never walks a tree itself.
2. **Expansion is incremental.** Discovered files are enqueued as ordinary
   queue jobs while enumeration continues; transfers may start before the walk
   finishes. Batch totals grow monotonically during expansion and the UI
   labels the batch as still counting until expansion completes.
3. **Members are ordinary jobs.** Every enqueued file is a normal durable
   `TransferJob` carrying the shared `batch_id` — same states, checkpoints,
   conflicts, retries, pipelined engine, scheduler limits, and destination
   locks as a single-file transfer. Nothing in the reducer, scheduler, runner,
   or recovery matrix changes.
4. **One stuck file never blocks its batch.** Per-file conflicts and source
   changes use the existing `Needs attention` flow; unrelated members keep
   running (this is queue product rule 5, restated as a batch guarantee).
5. **Directory structure is recreated faithfully, including empty
   directories.** Uploads `mkdir` remote directories during expansion
   (create-if-missing semantics; an already-existing directory is not an
   error). Downloads `create_dir_all` locally. Created directories are not
   removed on cancel.
6. **Batch truth is derived, plus a small persisted record.** A `BatchInfo`
   record persists id, user-visible name (the source folder's name),
   direction, expansion state, and discovered totals. Everything else — done
   counts, aggregate speed, ETA — is derived from member jobs at event time
   and never persisted.
7. **Restart honesty.** After a restart, enqueued members restore exactly as
   the queue design dictates (paused, no network). A batch whose expansion
   was interrupted restores with expansion state `Interrupted`: its
   already-enqueued members are intact and resumable, the un-enumerated
   remainder is not re-discovered, and the batch header says so. Re-running
   the same folder transfer is the recovery path (existing per-file conflict
   handling makes it converge).
8. **Drag-and-drop feeds the same two commands.** A drop enqueues via the
   existing single-file command or the new recursive command — no third
   transfer path. OS-file drops use Tauri's native drag-drop event (the DOM
   never sees real paths); intra-app drags use plain DOM drag events.
9. **Symlinks are not followed.** Symlinked directories are skipped with a
   per-batch note (following them risks cycles and surprise trees); symlinked
   files are skipped the same way. A future browsing project may revisit.
10. **Hidden files are included.** A folder transfer copies the folder as it
    is on disk, independent of the panes' hidden-file display toggle.

## Architecture

### Expansion task

`transfer_enqueue_recursive(pane_id, direction, source_path, dest_path)`:

1. Validate the source is a directory; mint a batch id; persist `BatchInfo`
   with expansion state `Running` and the folder's basename as the batch name.
2. Walk breadth-first. Uploads walk the local filesystem
   (`tokio::fs::read_dir`); downloads walk the remote tree over the existing
   SFTP session (`list_dir`), reusing the session-registry connection the
   panes already hold — expansion never authenticates.
3. For each directory (in walk order, parents first): create the counterpart
   directory at the destination. For each file: enqueue a normal job with the
   batch id, mapped destination path, origin `filesPanel`, and the standard
   conflict policy.
4. Update `BatchInfo` discovered totals as entries are found (coalesced with
   the store's existing write cadence).
5. On completion, set expansion state `Complete`. On enumeration error
   (permission denied on a subdirectory, dropped connection), set
   `Interrupted` with a reason string; already-enqueued members are
   unaffected.

Concurrency: one expansion task per command invocation; expansions for
different folders may run concurrently. Expansion does not consume a
scheduler slot — it is I/O-light listing, and holding transfer slots hostage
to a slow walk would idle the link.

Depth/size bounds: none by design (a folder is whatever it is), but the walk
yields between directories so a pathological tree cannot starve the runtime.

### Batch persistence and events

- `BatchInfo { id, name, direction, expansion: Running | Complete |
  Interrupted { reason }, discovered_files, discovered_bytes, created_at }`
  joins the persisted store as an additive v1 field (`#[serde(default)]`
  empty map for older stores; no schema bump). Terminal-batch records are
  compacted away when their last member leaves history.
- Snapshot and job-delta events gain a `batches` map projection. Per-batch
  derived aggregates ride the projection, computed at emission time from
  member jobs: `files_done`, `files_total` (= discovered), `bytes_done`
  (sum of member frontiers), `bytes_total` (= discovered bytes),
  `speed_bytes_per_second` (sum of active member speeds, null when none),
  `eta_seconds` (remaining ÷ speed, null when speed is null).
- Event coalescing rules are unchanged; batch aggregates piggyback on events
  the queue already emits rather than adding a new event stream.

### New queue commands

- `transfer_enqueue_recursive` — described above; returns the batch id.
- `transfer_cancel_batch(batch_id)` — cancels every non-terminal member via
  the existing per-job cancel path and stops a still-running expansion for
  that batch. No new reducer transitions.

### Transfer Center: batch grouping

- Rows sharing a batch id render grouped under a batch header row: name and
  direction, `files_done/files_total` (suffixed `+` while expansion is
  `Running`, and an "expansion interrupted" marker when `Interrupted`),
  bytes progress, aggregate speed, ETA, and a Cancel-batch action (existing
  confirm dialog pattern).
- The header is presentation only — it holds no domain state and derives
  everything from the `batches` projection. Selection and keyboard
  navigation treat member rows exactly as today; the header itself is
  focusable only for its cancel action.
- Both layouts: in the wide table the header is a full-width grouping row;
  in the narrow card layout it is a header card above its member cards.
  Batchless jobs render exactly as today.
- History view: a completed batch groups the same way; clearing completed
  history drops batch records whose members were all cleared.

### Drag-and-drop

Two sources, one sink:

- **Between panes (DOM):** file and folder rows are `draggable`. A drag from
  one pane marks the opposite pane's listing area as the drop target
  (visible drop highlight; the target directory is that pane's current
  path). Drop → folder rows call the recursive command, file rows call the
  existing single-file command. Dropping a remote item on the remote pane
  (or local on local) is a no-op in v1 — no intra-pane move semantics.
- **From the OS (native):** the webview listens to Tauri v2's drag-drop
  event, which supplies real filesystem paths and a drop position. The drop
  position is hit-tested against the remote pane's listing rect; drops
  elsewhere are ignored. Each dropped path enqueues an upload into the
  remote pane's current directory (directory → recursive, file → single).
  A drop while the remote pane has no session shows the existing
  "not connected" toast and does nothing.
- Drop feedback: the existing drop-highlight visual language (border/tint on
  the target listing) using theme tokens; the Transfer Center auto-open
  behavior applies unchanged since drops enqueue `filesPanel`-origin jobs.

### Files-panel entry points

The pane row context menu gains "Upload folder" / "Download folder" on
directory rows (label matched to pane direction), calling the recursive
command with the row's path and the opposite pane's current directory.
Existing file-row Upload/Download items are unchanged.

## Failure behavior

- Source folder missing or unreadable at expansion start: the command errors;
  no batch is created; existing toast surfaces it.
- Enumeration error mid-walk: batch expansion state `Interrupted { reason }`;
  enqueued members unaffected; header shows the marker.
- Directory creation failure (permission denied at destination): expansion
  interrupts with that reason; files already enqueued under directories that
  do exist continue.
- Per-file failures, conflicts, retries: existing queue behavior, member by
  member.
- Restart mid-expansion: `Running` restores as `Interrupted` ("app closed
  during expansion"); members restore per the queue design.
- Cancel batch during expansion: expansion stops, members cancel via the
  existing path, created directories remain.
- Store with batches from a newer schema: the existing quarantine rules
  apply unchanged.

## Testing strategy

### Rust unit tests

- Path mapping: source-tree path → destination path for both directions,
  including nested, empty, unicode, and trailing-slash cases.
- Expansion walker against fake filesystems/SFTP listings: ordering
  (parents before children), empty directories created, symlinks skipped
  with a recorded note, hidden files included, incremental enqueue calls,
  interrupted-walk state transitions.
- BatchInfo serde: additive default for older stores, round-trip, compaction
  when the last member clears.
- Aggregate derivation: files/bytes done, speed summation (nulls ignored),
  ETA math, `+` semantics while expansion runs — pure functions over job
  fixtures.
- Cancel-batch: cancels exactly the batch's non-terminal members and stops
  its expansion; other batches untouched.

### Rust integration tests

- Recursive upload and download against the fake transfer IO: tree arrives
  complete, directories (including empty) exist, every member carries the
  batch id, batch reaches `Complete` with matching totals.
- One member forced into `Needs attention`: siblings finish; batch aggregate
  reflects the stuck member; resolving it completes the batch.
- Restart mid-expansion: `Interrupted` restored, members paused, no network.

### Frontend tests

- Batch header rendering in both layouts (counts, `+` marker, interrupted
  marker, cancel action) from snapshot fixtures; batchless rows unchanged.
- Grouping stability under member updates and history clearing.
- DnD: drop-target resolution and command routing (folder → recursive,
  file → single, wrong-pane and no-session guards) with synthesized events;
  the native-drop hit-test given a fake drop position.

### Live verification (opt-in harness)

- Round-trip a fixture tree (nested + empty dirs + unicode names) against
  the live server: upload recursively, download recursively to a fresh
  location, diff the trees byte-for-byte.

### Manual checklist additions

- Drag a folder from Finder onto the remote pane; drag files between panes
  both directions; cancel a large batch mid-flight; kill the app
  mid-expansion and inspect the interrupted batch after relaunch.

## Non-goals

- Multi-select batches (next slice; the batch model built here is the
  foundation for it).
- Dragging out of the app to the OS.
- Batch-level conflict resolution ("apply to all") — per-file for now.
- Directory comparison, sync, or mirror.
- Move semantics, intra-pane drag reordering, or drag-to-breadcrumb.
- Removing created directories on cancel.
- Following symlinks.

## Success criteria

1. Right-clicking a folder in either pane transfers the whole tree through
   the queue, recreating structure including empty directories, with every
   file resumable, conflict-isolated, and pipelined like any single transfer.
2. The Transfer Center shows one honest roll-up per batch — files, bytes,
   combined speed, ETA — that keeps counting during expansion, survives
   restart as `Interrupted` without lying about totals, and can cancel the
   whole batch.
3. Dropping a folder from the OS onto the remote pane uploads it
   recursively into the pane's directory; dragging rows between panes
   transfers them; no third transfer pathway exists.
4. A stuck member never stalls its siblings or the queue.
5. Older persisted stores load unchanged; the store with batches loads in
   older readers' terms as plain jobs (additive field only).
