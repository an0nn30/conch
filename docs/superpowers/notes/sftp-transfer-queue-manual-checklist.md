# SFTP durable transfer queue manual verification

Use this checklist on a disposable OpenSSH/SFTP server. Attach screenshots,
logs, persisted queue excerpts, or screen recordings to each checked item; do
not record credentials, private-key material, or reusable server secrets.

## Evidence record

- Date/time (including timezone):
- Platform and OS version:
- TermLab build/commit:
- Branch:
- Server hostname or disposable environment identifier:
- Server OS and OpenSSH version (`sshd -V` or package version):
- Network conditions relevant to interruption testing:
- Tester:

## Source freshness

- [ ] Open TermLab, then modify a local source outside the app. Upload it,
  modify it again, and upload it a second time. Confirm both transfers read the
  bytes and metadata present when that transfer started, not a cached handle.
  Evidence:
- [ ] Open TermLab, then modify a remote source outside the app. Download it,
  modify it again, and download it a second time. Confirm both transfers read
  the current remote bytes and mtime-derived identity. Evidence:

## Durable pause, restart, and reconnect

- [ ] Relaunch with at least two formerly active jobs. Confirm no network work
  starts. Resume one row and confirm only it runs; relaunch again and choose
  `Resume All`, confirming every restored `Paused` row becomes runnable in one
  queue update. While startup remains suspended, enqueue a fresh editor/file
  transfer and confirm it runs without releasing unrelated restored rows.
  Evidence:

- [ ] Upload: pause after measurable progress, quit TermLab, relaunch, and
  confirm the queue is suspended and performs no network work. Explicitly
  reconnect, resume, and verify the final bytes and resumed offset. Evidence:
- [ ] Download: repeat pause → quit → relaunch → no network → explicit
  reconnect → resume, verifying the final bytes and resumed offset. Evidence:
- [ ] While an upload is paused, change its local source. Resume and confirm
  TermLab requires `Restart`; verify Resume cannot append the changed source to
  the old partial. Evidence:
- [ ] While a download is paused, change its remote source. Resume and confirm
  TermLab requires `Restart`; verify Resume cannot append the changed source to
  the old partial. Evidence:
- [ ] Remove a queued upload's local source before it starts. Confirm it enters
  `Needs attention` as Source missing without automatic retry and offers only
  Restart/Skip. Restore the exact source path, choose Restart, and verify a
  fresh checking pass completes. Repeat by removing a queued remote download
  source and restoring it on the SFTP server. Evidence:

## Conflicts and independent work

- [ ] Exercise `Overwrite`, `Rename`, and `Skip` in both directions. Confirm
  the requested result, that no unrelated destination changes, and that owned
  partial/backup files are removed after success. Evidence:
- [ ] Repeat Rename and Skip with an owned partial/backup or interrupted commit.
  Confirm the old destination and managed-artifact identity remain persisted
  until cleanup acknowledges. Inject a cleanup failure and confirm the row
  remains `Needs attention`, names the exact leftover artifact, and rejects a
  competing resolution. Evidence:
- [ ] Exercise compatible `Resume` with a durable partial. While the conflict
  is awaiting a choice, verify unrelated queued jobs continue within the
  configured limits. Evidence:

## Scheduling and serialization

- [ ] From two windows, enqueue enough transfers across at least two hosts to
  verify a global limit of 3 and per-host limit of 2. Record simultaneous jobs
  and confirm completing one host does not release another host's slot.
  Evidence:
- [ ] Enqueue two jobs targeting the same canonical destination from different
  windows. Confirm destination serialization permits only one owner at a time.
  Evidence:
- [ ] From two different remote hosts, download to the same normalized local
  destination and confirm only one attempt owns it at a time. Then download to
  two distinct local destinations and confirm cross-host work may overlap
  within the global limit. Evidence:

## Commit interruption recovery

For each row, begin with an intact old destination, interrupt at the named
durable phase, relaunch suspended, inspect final/partial/backup inventory, then
explicitly recover. Record which intact old or new file remains.

- [ ] `Prepared`: Evidence:
- [ ] `BackupMoved`: Evidence:
- [ ] `PartialPromoted`: Evidence:
- [ ] `CleanupPending`: Evidence:
- [ ] `Complete`: Evidence:
- [ ] For any ambiguous inventory, confirm all artifacts are preserved and the
  job enters `Needs attention` instead of guessing ownership. Evidence:

## Transfer Center desktop UX

- [ ] Dock, resize narrow/wide, hide/show, and pop out the Transfer Center.
  Confirm dense rows remain readable, scrolling is contained, and shell chrome
  is stable. Evidence:
- [ ] Navigate Active/History, rows, and actions with the keyboard. Confirm
  deterministic focus, visible focus indication, meaningful progress names,
  and selection following the visible projection. Evidence:
- [ ] Verify loading, empty, recovery, connection, conflict, retry, permission,
  disk-full, cleanup, failed, cancelled, and completed feedback. Confirm errors
  identify actionable paths without exposing secrets. Evidence:

## Credential and persistence inspection

- [ ] Inspect the persisted transfer JSON while queued, running, paused,
  retrying, awaiting attention, completed, and cancelled. Confirm it contains
  endpoint identity and managed paths but no password, passphrase, private key,
  SSH handle, or session handle. Evidence:
- [ ] Inspect application logs for the same lifecycle. Confirm credentials and
  private-key material never appear, including in failure messages. Evidence:

## FileZilla gap reconciliation

Record product-gap conclusions here; do not edit a local, untracked
`SFTP_FILEZILLA_GAP.md` during implementation reconciliation.

Implemented in this project:

- [ ] Durable upload/download queue and persisted history
- [ ] Pause/resume with source identity and durable checkpoints
- [ ] Global/per-host concurrency and destination serialization
- [ ] Bounded automatic retry with explicit reconnect/retry
- [ ] Conflict handling (`Overwrite`, `Rename`, `Skip`, compatible `Resume`)
- [ ] Transfer Center Active/History desktop surface

Later projects, not claims of this implementation:

- [ ] Recursive directory transfer
- [ ] Drag-and-drop transfer workflows
- [ ] Directory synchronization/comparison
- [ ] Advanced remote browsing features
- [ ] Broader resilience work beyond the documented crash/retry model
- [ ] Protocols other than SFTP

## Automated hardening reconciliation

These deferred review items were resolved or dispositioned before the design
status changed to Implemented:

- Remote partial open flags have a direct seam test: resume preserves content,
  while a fresh transfer adds truncate. Upload and download wrappers both use
  the shared resume-offset helper.
- A short-writing async destination proves `write_all` retries until the entire
  chunk is stored.
- The previously reported duplicate `std::path::Path` import is obsolete: the
  production module has one combined path import and the test-only import is
  independently required by its test module.
- The atomic enqueue-order regression records the persisted revision and
  verifies delta, legacy, and summary emission all occur after persistence.
- A panicking runner becomes a durable permanent failure and its lease is
  released; a second job starts at global capacity one.
- A two-host scheduler regression proves that one host's completion does not
  release another host's occupied per-host slot.
- Commit execution delegates phase/inventory decisions to the authoritative
  `recovery_action` matrix. The sole extension is an explicitly authorized
  Overwrite response to a late final after `BackupMoved`; the next inventory
  returns to the shared matrix.
- Unmeasured speed and ETA are nullable/unknown rather than zero. Startup
  recovery clears stale instantaneous values.
- Transfer Center selection consumes the renderer's public `jobsFor`
  projection, eliminating the controller's duplicate terminal-state set.
- Progress accessible names use the visible filename/path, and the dense-table
  minimum width is a named component token.
- Task 12 cancellation/recovery and lease suites remain in the full green gate;
  they were not mechanically duplicated.

## Pipelined transfers

Covers the [pipelined SFTP transfers design](../specs/2026-08-25-sftp-pipelined-transfers-design.md).
The two live tests below (`crates/termlab_tauri/src/remote/transfer_queue/runner.rs`,
`#[ignore]`, gated on `TERMLAB_TEST_SFTP_HOST`/`_PORT`/`_USER`/`_KEY`) were
invoked in the Task 8 environment and skipped cleanly before connecting
because no disposable OpenSSH server was configured; both rows remain
pending-live-evidence until run against one.

- [ ] `live_pipelined_upload_matches_content_and_beats_sequential`: upload a
  64 MiB fixture once at `PipelineTuning { depth: 1, chunk_bytes: 262144 }` and
  once at `{ depth: 16, chunk_bytes: 262144 }` to distinct remote paths.
  Confirm both remote files match the source byte-for-byte and record the
  measured depth-1 vs. depth-16 throughput (elapsed time and MiB/s, printed by
  the test). Evidence: pending-live-evidence (skipped: no
  `TERMLAB_TEST_SFTP_*` server configured in this environment).
- [ ] `live_pipelined_download_resumes_after_interruption`: start a pipelined
  download (`depth: 8`) with a control callback that pauses once the
  contiguous frontier passes 25% of the source; confirm the outcome is
  `Paused` with a frontier strictly between 0 and the total, resume with
  `offset = frontier`, and confirm completion plus a byte-for-byte match
  against the remote source. Evidence: pending-live-evidence (skipped: no
  `TERMLAB_TEST_SFTP_*` server configured in this environment).

## Sign-off

- Overall result: [ ] Pass [ ] Pass with accepted deviations [ ] Fail
- Accepted deviations and linked issues:
- Evidence bundle location:
- Reviewer/date:
