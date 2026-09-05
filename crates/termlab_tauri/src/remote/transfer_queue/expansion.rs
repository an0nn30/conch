use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::batch::{BATCH_CANCELLED_REASON, BatchCancellation, BatchExpansion};

/// Map a discovered source entry onto its destination path.
/// POSIX remote paths use '/'; local paths use the platform separator.
pub fn map_destination(
    source_root: &str,
    dest_root: &str,
    entry_path: &str,
    remote_dest: bool,
) -> Result<String, String> {
    let src = normalize_root(source_root);
    let dest = normalize_root(dest_root);

    let relative: &str = if entry_path == src {
        ""
    } else {
        let prefix = if src == "/" {
            "/".to_string()
        } else {
            format!("{src}/")
        };
        match entry_path.strip_prefix(prefix.as_str()) {
            Some(rest) => rest,
            None => {
                return Err(format!(
                    "entry path '{entry_path}' is not under source root '{source_root}'"
                ));
            }
        }
    };

    if remote_dest {
        Ok(join_posix(&dest, relative))
    } else {
        let mut path = PathBuf::from(&dest);
        if !relative.is_empty() {
            for segment in relative.split('/') {
                path.push(segment);
            }
        }
        Ok(path.to_string_lossy().into_owned())
    }
}

/// Trim a trailing '/' from a root path, collapsing back to "/" if that
/// leaves nothing (i.e. the root itself was "/").
fn normalize_root(root: &str) -> String {
    let trimmed = root.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn join_posix(dest: &str, relative: &str) -> String {
    if relative.is_empty() {
        dest.to_string()
    } else if dest == "/" {
        format!("/{relative}")
    } else {
        format!("{dest}/{relative}")
    }
}

/// One entry the walk found. `SkippedOther` covers anything with no bytes to
/// transfer — a fifo, socket, or device node; the walker treats it exactly
/// like a skipped symlink, and it exists only so the per-batch note can say
/// what was actually skipped.
#[derive(Debug, Clone, PartialEq)]
pub enum WalkEntry {
    Dir { path: String },
    File { path: String, size: u64 },
    SkippedSymlink { path: String },
    SkippedOther { path: String },
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
    C: Fn() -> bool,
{
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(root.to_string());

    while let Some(dir) = queue.pop_front() {
        if cancelled() {
            return Ok(());
        }

        let children = lister
            .list(&dir)
            .await
            .map_err(|err| format!("{dir}: {err}"))?;

        for entry in children {
            if cancelled() {
                return Ok(());
            }

            if let WalkEntry::Dir { path } = &entry {
                queue.push_back(path.clone());
            }

            on_entry(entry).await?;
        }

        tokio::task::yield_now().await;
    }

    Ok(())
}

/// A file the walk found, already mapped onto its destination path.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredFile {
    pub source_path: String,
    pub dest_path: String,
    pub size: u64,
}

/// What expansion has found so far. Totals only ever grow, which is what lets
/// the UI show them as a running count while the walk continues.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExpansionTotals {
    pub discovered_files: u64,
    pub discovered_bytes: u64,
    pub skipped: Vec<String>,
}

/// Creates a directory at the destination side of a transfer. Implementations
/// must treat an existing directory as success (product rule 5).
#[async_trait]
pub trait DirectoryCreator: Send + Sync {
    async fn create_dir(&self, path: &str) -> Result<(), String>;
}

/// Where expansion delivers what it finds: member jobs into the queue, totals
/// and expansion state onto the batch record.
#[async_trait]
pub trait ExpansionSink: Send + Sync {
    /// Deliver a chunk of discovered files. Chunks arrive once per
    /// coalescing window — never once per file — so a sink backed by the
    /// durable queue commits once per chunk. A chunk is accepted or refused
    /// whole.
    async fn enqueue_files(&self, files: Vec<DiscoveredFile>) -> Result<(), String>;

    async fn record_batch(
        &self,
        expansion: BatchExpansion,
        totals: &ExpansionTotals,
    ) -> Result<(), String>;
}

/// The source tree and where it lands.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpansionPlan {
    pub source_root: String,
    pub dest_root: String,
    /// The destination is the remote side — an upload.
    pub remote_dest: bool,
}

/// Entries walked between two sink deliveries. Expansion can discover
/// thousands of files, and every delivery is a full durable document commit,
/// so both the member enqueues and the totals are coalesced into one chunk
/// per window instead of one commit per entry.
const UPDATE_COALESCE_ENTRIES: u64 = 25;

#[derive(Default)]
struct ExpansionState {
    totals: ExpansionTotals,
    since_update: u64,
    /// Files discovered but not yet delivered to the sink.
    pending: Vec<DiscoveredFile>,
}

type EntryFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Walk `plan.source_root`, recreating its directories at the destination and
/// enqueuing every file as a member of the caller's batch. Returns the final
/// expansion state, which has already been handed to `sink`.
///
/// This function performs no authentication and consumes no scheduler slot:
/// the lister and the directory creator are handed an already-open session by
/// the caller, and members run as ordinary queue jobs.
pub async fn run_expansion(
    lister: &dyn TreeLister,
    creator: &dyn DirectoryCreator,
    sink: &dyn ExpansionSink,
    plan: &ExpansionPlan,
    cancellation: &BatchCancellation,
) -> BatchExpansion {
    let state = Mutex::new(ExpansionState::default());
    let root = normalize_entry_path(&plan.source_root);

    // The walk only ever emits the root's descendants, so the counterpart of
    // the source folder itself is created here. It is also what makes
    // transferring an empty folder produce an empty folder.
    let walked = match creator.create_dir(&plan.dest_root).await {
        Ok(()) => {
            walk_tree(
                lister,
                &root,
                |entry| -> EntryFuture<'_> {
                    Box::pin(expand_entry(entry, creator, sink, plan, &state))
                },
                || cancellation.is_cancelled(),
            )
            .await
        }
        Err(error) => Err(error),
    };

    // Deliver whatever the walk buffered after its last flush. A cancelled
    // batch would refuse the members anyway, so the leftovers are dropped.
    let walked = if cancellation.is_cancelled() {
        walked
    } else {
        let leftover = std::mem::take(&mut state.lock().pending);
        match (walked, flush_files(sink, leftover).await) {
            (Ok(()), Err(reason)) => Err(reason),
            (walked, _) => walked,
        }
    };

    let totals = state.lock().totals.clone();
    // Cancellation outranks whatever the walk's proximate error was: a cancel
    // that lands mid-flight makes the queue refuse the member the walk was
    // already enqueuing, and "batch cancelled" is the honest reason for that
    // rejection rather than the rejection's own wording.
    let expansion = if cancellation.is_cancelled() {
        BatchExpansion::Interrupted {
            reason: BATCH_CANCELLED_REASON.to_string(),
        }
    } else {
        match walked {
            Ok(()) => BatchExpansion::Complete,
            Err(reason) => BatchExpansion::Interrupted { reason },
        }
    };

    if let Err(error) = sink.record_batch(expansion.clone(), &totals).await {
        log::error!("could not record the final expansion state: {error}");
    }
    expansion
}

async fn expand_entry(
    entry: WalkEntry,
    creator: &dyn DirectoryCreator,
    sink: &dyn ExpansionSink,
    plan: &ExpansionPlan,
    state: &Mutex<ExpansionState>,
) -> Result<(), String> {
    match entry {
        WalkEntry::Dir { path } => {
            let destination = map_entry(plan, &path)?;
            creator.create_dir(&destination).await?;
        }
        WalkEntry::File { path, size } => {
            let destination = map_entry(plan, &path)?;
            let mut state = state.lock();
            state.pending.push(DiscoveredFile {
                source_path: normalize_entry_path(&path),
                dest_path: destination,
                size,
            });
            state.totals.discovered_files += 1;
            state.totals.discovered_bytes = state.totals.discovered_bytes.saturating_add(size);
        }
        // The note is what the user reads, so it says what was skipped rather
        // than leaving a bare path that looks like an ordinary omission.
        WalkEntry::SkippedSymlink { path } => {
            state.lock().totals.skipped.push(format!("symlink: {path}"));
        }
        WalkEntry::SkippedOther { path } => {
            state
                .lock()
                .totals
                .skipped
                .push(format!("special file: {path}"));
        }
    }

    let due = {
        let mut state = state.lock();
        state.since_update += 1;
        state.since_update >= UPDATE_COALESCE_ENTRIES
    };
    if due {
        let (files, totals) = {
            let mut state = state.lock();
            state.since_update = 0;
            (std::mem::take(&mut state.pending), state.totals.clone())
        };
        // Members land before the totals that count them, so the batch
        // record never reports files the queue has not accepted yet.
        flush_files(sink, files).await?;
        sink.record_batch(BatchExpansion::Running, &totals).await?;
    }
    Ok(())
}

async fn flush_files(sink: &dyn ExpansionSink, files: Vec<DiscoveredFile>) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    sink.enqueue_files(files).await
}

fn map_entry(plan: &ExpansionPlan, entry_path: &str) -> Result<String, String> {
    map_destination(
        &plan.source_root,
        &plan.dest_root,
        &normalize_entry_path(entry_path),
        plan.remote_dest,
    )
}

/// `map_destination` matches entry paths against the source root literally, so
/// a lister that reports `/src/a/` instead of `/src/a` would map onto a
/// destination with a stray trailing separator. Trim it here, once.
fn normalize_entry_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use uuid::Uuid;

    use super::super::batch::{BATCH_CANCELLED_REASON, BatchExpansion, BatchInfo};
    use super::super::engine::{QueueActor, TransferQueueHandle};
    use super::super::events::{QueueEventPayload, QueueSummaryPayload, TransferEventSink};
    use super::super::model::{
        ConflictPolicy, NewTransferJob, TransferDirection, TransferEndpoint, TransferJobState,
        TransferOrigin, TransferPriority, TransferProtocol, build_destination_key, build_host_key,
    };
    use super::super::store::TransferStore;

    struct FakeLister {
        tree: BTreeMap<String, Vec<WalkEntry>>,
        errors: BTreeMap<String, String>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeLister {
        fn new(tree: BTreeMap<String, Vec<WalkEntry>>) -> Self {
            Self {
                tree,
                errors: BTreeMap::new(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn with_error(mut self, path: &str, message: &str) -> Self {
            self.errors.insert(path.to_string(), message.to_string());
            self
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl TreeLister for FakeLister {
        async fn list(&self, path: &str) -> Result<Vec<WalkEntry>, String> {
            self.calls.lock().unwrap().push(path.to_string());
            if let Some(message) = self.errors.get(path) {
                return Err(message.clone());
            }
            Ok(self.tree.get(path).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn maps_nested_paths_both_separators() {
        // Basic POSIX join.
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b/c/d.txt", true).unwrap(),
            "/home/x/c/d.txt"
        );

        // Same relative path, platform join.
        let expected_platform = {
            let mut path = PathBuf::from("/home/x");
            path.push("c");
            path.push("d.txt");
            path.to_string_lossy().into_owned()
        };
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b/c/d.txt", false).unwrap(),
            expected_platform
        );

        // The root itself maps straight to dest_root.
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b", true).unwrap(),
            "/home/x"
        );

        // Entries outside the source root are rejected.
        assert!(map_destination("/a/b", "/home/x", "/a/c/d.txt", true).is_err());

        // Unicode path segments are preserved verbatim.
        assert_eq!(
            map_destination("/a/b", "/home/x", "/a/b/café/döc.txt", true).unwrap(),
            "/home/x/café/döc.txt"
        );

        // Trailing slashes on both roots are normalized away.
        assert_eq!(
            map_destination("/a/b/", "/home/x/", "/a/b/c/d.txt", true).unwrap(),
            "/home/x/c/d.txt"
        );
    }

    fn three_level_tree() -> BTreeMap<String, Vec<WalkEntry>> {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/root".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/root/a".into(),
                },
                WalkEntry::File {
                    path: "/root/f0.txt".into(),
                    size: 10,
                },
                WalkEntry::Dir {
                    path: "/root/b".into(),
                },
            ],
        );
        tree.insert(
            "/root/a".to_string(),
            vec![
                WalkEntry::File {
                    path: "/root/a/f1.txt".into(),
                    size: 1,
                },
                WalkEntry::Dir {
                    path: "/root/a/aa".into(),
                },
            ],
        );
        tree.insert(
            "/root/b".to_string(),
            vec![WalkEntry::File {
                path: "/root/b/f2.txt".into(),
                size: 2,
            }],
        );
        tree.insert(
            "/root/a/aa".to_string(),
            vec![WalkEntry::File {
                path: "/root/a/aa/f3.txt".into(),
                size: 3,
            }],
        );
        tree
    }

    #[tokio::test]
    async fn walk_emits_parents_before_children_breadth_first() {
        let lister = FakeLister::new(three_level_tree());
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        walk_tree(
            &lister,
            "/root",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await
        .unwrap();

        let emitted = emitted.into_inner();
        assert_eq!(
            emitted,
            vec![
                WalkEntry::Dir {
                    path: "/root/a".into()
                },
                WalkEntry::File {
                    path: "/root/f0.txt".into(),
                    size: 10
                },
                WalkEntry::Dir {
                    path: "/root/b".into()
                },
                WalkEntry::File {
                    path: "/root/a/f1.txt".into(),
                    size: 1
                },
                WalkEntry::Dir {
                    path: "/root/a/aa".into()
                },
                WalkEntry::File {
                    path: "/root/b/f2.txt".into(),
                    size: 2
                },
                WalkEntry::File {
                    path: "/root/a/aa/f3.txt".into(),
                    size: 3
                },
            ]
        );
    }

    #[tokio::test]
    async fn walk_includes_empty_dirs_and_hidden_files() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/r/empty".into(),
                },
                WalkEntry::File {
                    path: "/r/.hidden".into(),
                    size: 5,
                },
            ],
        );
        tree.insert("/r/empty".to_string(), Vec::new());
        let lister = FakeLister::new(tree);
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await
        .unwrap();

        assert_eq!(
            emitted.into_inner(),
            vec![
                WalkEntry::Dir {
                    path: "/r/empty".into()
                },
                WalkEntry::File {
                    path: "/r/.hidden".into(),
                    size: 5
                },
            ]
        );
        assert_eq!(lister.call_count(), 2, "empty dir must still be listed");
    }

    #[tokio::test]
    async fn walk_skips_symlinks_with_marker() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![WalkEntry::SkippedSymlink {
                path: "/r/link".into(),
            }],
        );
        let lister = FakeLister::new(tree);
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await
        .unwrap();

        assert_eq!(
            emitted.into_inner(),
            vec![WalkEntry::SkippedSymlink {
                path: "/r/link".into()
            }]
        );
        assert_eq!(
            lister.call_count(),
            1,
            "a skipped symlink must never be descended into"
        );
    }

    #[tokio::test]
    async fn walk_stops_when_cancelled() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![
                WalkEntry::File {
                    path: "/r/a.txt".into(),
                    size: 1,
                },
                WalkEntry::File {
                    path: "/r/b.txt".into(),
                    size: 2,
                },
                WalkEntry::Dir {
                    path: "/r/c".into(),
                },
                WalkEntry::File {
                    path: "/r/d.txt".into(),
                    size: 4,
                },
            ],
        );
        tree.insert(
            "/r/c".to_string(),
            vec![WalkEntry::File {
                path: "/r/c/e.txt".into(),
                size: 5,
            }],
        );
        let lister = FakeLister::new(tree);
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());
        let seen = RefCell::new(0usize);

        walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                *seen.borrow_mut() += 1;
                async { Ok(()) }
            },
            || *seen.borrow() >= 2,
        )
        .await
        .unwrap();

        assert_eq!(
            emitted.into_inner(),
            vec![
                WalkEntry::File {
                    path: "/r/a.txt".into(),
                    size: 1
                },
                WalkEntry::File {
                    path: "/r/b.txt".into(),
                    size: 2
                },
            ]
        );
        assert_eq!(
            lister.call_count(),
            1,
            "cancellation must stop further lister.list calls"
        );
    }

    #[tokio::test]
    async fn walk_surfaces_lister_error_with_path() {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/r".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/r/bad".into(),
                },
                WalkEntry::File {
                    path: "/r/ok.txt".into(),
                    size: 1,
                },
            ],
        );
        let lister = FakeLister::new(tree).with_error("/r/bad", "permission denied");
        let emitted: RefCell<Vec<WalkEntry>> = RefCell::new(Vec::new());

        let result = walk_tree(
            &lister,
            "/r",
            |entry| {
                emitted.borrow_mut().push(entry);
                async { Ok(()) }
            },
            || false,
        )
        .await;

        let err = result.expect_err("subdirectory listing failure must propagate");
        assert!(
            err.contains("/r/bad"),
            "error must mention the failing path: {err}"
        );
        assert_eq!(
            emitted.into_inner(),
            vec![
                WalkEntry::Dir {
                    path: "/r/bad".into()
                },
                WalkEntry::File {
                    path: "/r/ok.txt".into(),
                    size: 1
                },
            ],
            "entries discovered before the failure must already be emitted"
        );
    }

    // -----------------------------------------------------------------------
    // run_expansion against the real queue actor
    // -----------------------------------------------------------------------

    struct NoopEventSink;

    #[async_trait]
    impl TransferEventSink for NoopEventSink {
        async fn job_updated(&self, _payload: QueueEventPayload) {}

        async fn queue_summary(&self, _payload: QueueSummaryPayload) {}

        async fn legacy_progress(&self, _payload: termlab_remote::transfer::TransferProgress) {}
    }

    #[derive(Default)]
    struct RecordingDirCreator {
        created: Mutex<Vec<String>>,
    }

    impl RecordingDirCreator {
        fn created(&self) -> Vec<String> {
            self.created.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DirectoryCreator for RecordingDirCreator {
        async fn create_dir(&self, path: &str) -> Result<(), String> {
            self.created.lock().unwrap().push(path.to_string());
            Ok(())
        }
    }

    /// Enqueues every discovered chunk as upload members of `batch_id` and
    /// mirrors expansion state onto the batch record, exactly as the Tauri
    /// expansion task does.
    struct QueueSink {
        handle: TransferQueueHandle,
        batch_id: Uuid,
        /// Cancels the batch right after the first chunk lands, which is how
        /// the cancellation test reaches a mid-walk cancel deterministically.
        cancel_after_first_chunk: bool,
        /// Cancels the batch *before* the first chunk is attempted, staging
        /// the hostile mailbox order: the walk decided to enqueue, the actor
        /// processed the cancel first, and the enqueue arrives afterwards.
        cancel_before_first_chunk: bool,
        enqueued: Mutex<Vec<Uuid>>,
    }

    impl QueueSink {
        fn new(handle: TransferQueueHandle, batch_id: Uuid) -> Self {
            Self {
                handle,
                batch_id,
                cancel_after_first_chunk: false,
                cancel_before_first_chunk: false,
                enqueued: Mutex::new(Vec::new()),
            }
        }

        fn cancelling_after_first_chunk(mut self) -> Self {
            self.cancel_after_first_chunk = true;
            self
        }

        fn cancelling_before_first_chunk(mut self) -> Self {
            self.cancel_before_first_chunk = true;
            self
        }
    }

    #[async_trait]
    impl ExpansionSink for QueueSink {
        async fn enqueue_files(&self, files: Vec<DiscoveredFile>) -> Result<(), String> {
            let first_chunk = self.enqueued.lock().unwrap().is_empty();
            if self.cancel_before_first_chunk && first_chunk {
                self.handle.cancel_batch(self.batch_id).await?;
            }
            let endpoint = TransferEndpoint::Configured {
                server_entry_id: "server-1".into(),
                label: "Production".into(),
            };
            let host_key = build_host_key(&endpoint);
            let requests: Vec<NewTransferJob> = files
                .into_iter()
                .map(|file| {
                    let destination_key = build_destination_key(
                        &host_key,
                        &TransferDirection::Upload,
                        &file.source_path,
                        &file.dest_path,
                    );
                    let file_name = file
                        .source_path
                        .rsplit('/')
                        .find(|component| !component.is_empty())
                        .unwrap_or(&file.source_path)
                        .to_string();
                    NewTransferJob {
                        id: Uuid::new_v4(),
                        protocol: TransferProtocol::Sftp,
                        direction: TransferDirection::Upload,
                        origin: TransferOrigin::FilesPanel,
                        endpoint: endpoint.clone(),
                        local_path: file.source_path.clone(),
                        remote_path: file.dest_path.clone(),
                        file_name,
                        batch_id: Some(self.batch_id),
                        priority: TransferPriority::Normal,
                        host_key: host_key.clone(),
                        destination_key,
                        conflict_policy: ConflictPolicy::Ask,
                    }
                })
                .collect();
            let ids = self.handle.enqueue_many(requests).await?;
            self.enqueued.lock().unwrap().extend(ids);
            if self.cancel_after_first_chunk && first_chunk {
                self.handle.cancel_batch(self.batch_id).await?;
            }
            Ok(())
        }

        async fn record_batch(
            &self,
            expansion: BatchExpansion,
            totals: &ExpansionTotals,
        ) -> Result<(), String> {
            self.handle
                .update_batch(
                    self.batch_id,
                    expansion,
                    totals.discovered_files,
                    totals.discovered_bytes,
                    totals.skipped.clone(),
                )
                .await
        }
    }

    async fn queue_handle() -> (tempfile::TempDir, TransferQueueHandle) {
        let directory = tempfile::tempdir().unwrap();
        let handle = QueueActor::spawn(
            TransferStore::new(directory.path().join("transfers.json")),
            Arc::new(NoopEventSink),
        )
        .unwrap();
        (directory, handle)
    }

    fn batch_info(id: Uuid) -> BatchInfo {
        BatchInfo {
            id,
            name: "src".into(),
            direction: TransferDirection::Upload,
            expansion: BatchExpansion::Running,
            discovered_files: 0,
            discovered_bytes: 0,
            skipped: Vec::new(),
            completed_files: 0,
            completed_bytes: 0,
            created_at_ms: 10,
        }
    }

    fn upload_plan() -> ExpansionPlan {
        ExpansionPlan {
            source_root: "/src".into(),
            dest_root: "/dst".into(),
            remote_dest: true,
        }
    }

    /// Two populated directories, one empty directory, three files (one of
    /// them hidden), and one symlink that must be skipped.
    fn expansion_tree() -> BTreeMap<String, Vec<WalkEntry>> {
        let mut tree = BTreeMap::new();
        tree.insert(
            "/src".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/src/a".into(),
                },
                WalkEntry::Dir {
                    path: "/src/empty".into(),
                },
                WalkEntry::File {
                    path: "/src/.top".into(),
                    size: 10,
                },
                WalkEntry::SkippedSymlink {
                    path: "/src/link".into(),
                },
            ],
        );
        tree.insert(
            "/src/a".to_string(),
            vec![
                WalkEntry::Dir {
                    path: "/src/a/nested".into(),
                },
                WalkEntry::File {
                    path: "/src/a/one.txt".into(),
                    size: 20,
                },
            ],
        );
        tree.insert("/src/empty".to_string(), Vec::new());
        tree.insert(
            "/src/a/nested".to_string(),
            vec![WalkEntry::File {
                path: "/src/a/nested/two.txt".into(),
                size: 30,
            }],
        );
        tree
    }

    #[tokio::test]
    async fn recursive_expansion_enqueues_batch_members_and_completes() {
        let (_directory, handle) = queue_handle().await;
        let batch_id = Uuid::from_u128(0xB1);
        let cancellation = handle.create_batch(batch_info(batch_id)).await.unwrap();
        let lister = FakeLister::new(expansion_tree());
        let creator = RecordingDirCreator::default();
        let sink = QueueSink::new(handle.clone(), batch_id);

        let outcome = run_expansion(&lister, &creator, &sink, &upload_plan(), &cancellation).await;

        assert_eq!(outcome, BatchExpansion::Complete);
        assert_eq!(
            creator.created(),
            vec![
                "/dst".to_string(),
                "/dst/a".to_string(),
                "/dst/empty".to_string(),
                "/dst/a/nested".to_string(),
            ],
            "the destination root comes first, then directories parents-first \
             including the empty one"
        );

        let snapshot = handle.snapshot();
        let mut destinations: Vec<_> = snapshot
            .jobs
            .iter()
            .map(|job| job.remote_path.clone())
            .collect();
        destinations.sort();
        assert_eq!(
            destinations,
            vec![
                "/dst/.top".to_string(),
                "/dst/a/nested/two.txt".to_string(),
                "/dst/a/one.txt".to_string(),
            ],
            "every discovered file becomes a member job at its mapped destination"
        );
        assert!(
            snapshot
                .jobs
                .iter()
                .all(|job| job.batch_id == Some(batch_id)),
            "every member carries the batch id"
        );

        assert_eq!(snapshot.batches.len(), 1);
        let info = &snapshot.batches[0].info;
        assert_eq!(info.expansion, BatchExpansion::Complete);
        assert_eq!(info.discovered_files, 3);
        assert_eq!(info.discovered_bytes, 60);
        assert_eq!(info.skipped, vec!["symlink: /src/link".to_string()]);
    }

    #[tokio::test]
    async fn expansion_error_interrupts_batch_but_keeps_members() {
        let (_directory, handle) = queue_handle().await;
        let batch_id = Uuid::from_u128(0xB2);
        let cancellation = handle.create_batch(batch_info(batch_id)).await.unwrap();
        let lister = FakeLister::new(expansion_tree()).with_error("/src/a", "permission denied");
        let creator = RecordingDirCreator::default();
        let sink = QueueSink::new(handle.clone(), batch_id);

        let outcome = run_expansion(&lister, &creator, &sink, &upload_plan(), &cancellation).await;

        let BatchExpansion::Interrupted { reason } = outcome else {
            panic!("a failed listing must interrupt expansion, got {outcome:?}");
        };
        assert!(
            reason.contains("/src/a"),
            "reason names the failure: {reason}"
        );

        let snapshot = handle.snapshot();
        assert_eq!(
            snapshot.jobs.len(),
            1,
            "the file discovered before the failure stays enqueued"
        );
        assert_eq!(snapshot.jobs[0].remote_path, "/dst/.top");
        assert_eq!(snapshot.jobs[0].batch_id, Some(batch_id));
        assert_eq!(snapshot.batches[0].info.discovered_files, 1);
        assert!(matches!(
            snapshot.batches[0].info.expansion,
            BatchExpansion::Interrupted { .. }
        ));
    }

    /// A root wide enough that the first coalescing window flushes mid-walk,
    /// before the trailing subdirectory is ever listed.
    fn wide_tree() -> BTreeMap<String, Vec<WalkEntry>> {
        let mut root: Vec<WalkEntry> = (0..super::UPDATE_COALESCE_ENTRIES)
            .map(|index| WalkEntry::File {
                path: format!("/src/f{index}.txt"),
                size: 1,
            })
            .collect();
        root.push(WalkEntry::Dir {
            path: "/src/sub".into(),
        });
        let mut tree = BTreeMap::new();
        tree.insert("/src".to_string(), root);
        tree.insert(
            "/src/sub".to_string(),
            vec![WalkEntry::File {
                path: "/src/sub/late.txt".into(),
                size: 1,
            }],
        );
        tree
    }

    #[tokio::test]
    async fn cancel_batch_cancels_members_and_stops_expansion() {
        let (_directory, handle) = queue_handle().await;
        let batch_id = Uuid::from_u128(0xB3);
        let cancellation = handle.create_batch(batch_info(batch_id)).await.unwrap();
        let lister = FakeLister::new(wide_tree());
        let creator = RecordingDirCreator::default();
        let sink = QueueSink::new(handle.clone(), batch_id).cancelling_after_first_chunk();

        let outcome = run_expansion(&lister, &creator, &sink, &upload_plan(), &cancellation).await;

        assert_eq!(
            outcome,
            BatchExpansion::Interrupted {
                reason: BATCH_CANCELLED_REASON.to_string(),
            }
        );
        assert_eq!(
            lister.call_count(),
            1,
            "cancellation stops the walk before any subdirectory is listed"
        );

        let snapshot = handle.snapshot();
        assert_eq!(
            snapshot.jobs.len(),
            super::UPDATE_COALESCE_ENTRIES as usize,
            "only the first flushed chunk was enqueued"
        );
        assert!(
            snapshot
                .jobs
                .iter()
                .all(|job| matches!(job.state, TransferJobState::Cancelled { .. })),
            "every member is cancelled through the existing bulk path"
        );
        assert_eq!(
            snapshot.batches[0].info.expansion,
            BatchExpansion::Interrupted {
                reason: BATCH_CANCELLED_REASON.to_string(),
            },
            "a late expansion update must not clobber the cancelled state"
        );
    }

    #[tokio::test]
    async fn a_cancel_that_lands_before_the_enqueue_still_refuses_the_member() {
        let (_directory, handle) = queue_handle().await;
        let batch_id = Uuid::from_u128(0xB7);
        let cancellation = handle.create_batch(batch_info(batch_id)).await.unwrap();
        let lister = FakeLister::new(expansion_tree());
        let creator = RecordingDirCreator::default();
        let sink = QueueSink::new(handle.clone(), batch_id).cancelling_before_first_chunk();

        let outcome = run_expansion(&lister, &creator, &sink, &upload_plan(), &cancellation).await;

        assert_eq!(
            outcome,
            BatchExpansion::Interrupted {
                reason: BATCH_CANCELLED_REASON.to_string(),
            },
            "the cancel outranks the enqueue rejection it caused"
        );

        let snapshot = handle.snapshot();
        assert!(
            snapshot.jobs.is_empty(),
            "a member decided on before the cancel must not slip in after it: {:?}",
            snapshot.jobs
        );
        assert_eq!(
            snapshot.batches[0].info.expansion,
            BatchExpansion::Interrupted {
                reason: BATCH_CANCELLED_REASON.to_string(),
            },
            "the rejection must not clobber the cancelled reason"
        );
    }

    #[tokio::test]
    async fn an_empty_source_folder_still_creates_its_destination() {
        let (_directory, handle) = queue_handle().await;
        let batch_id = Uuid::from_u128(0xB5);
        let cancellation = handle.create_batch(batch_info(batch_id)).await.unwrap();
        let mut tree = BTreeMap::new();
        tree.insert("/src".to_string(), Vec::new());
        let lister = FakeLister::new(tree);
        let creator = RecordingDirCreator::default();
        let sink = QueueSink::new(handle.clone(), batch_id);

        let outcome = run_expansion(&lister, &creator, &sink, &upload_plan(), &cancellation).await;

        assert_eq!(outcome, BatchExpansion::Complete);
        assert_eq!(creator.created(), vec!["/dst".to_string()]);
        assert!(handle.snapshot().jobs.is_empty());
    }

    #[tokio::test]
    async fn a_destination_that_cannot_be_created_interrupts_before_the_walk() {
        struct FailingDirCreator;

        #[async_trait]
        impl DirectoryCreator for FailingDirCreator {
            async fn create_dir(&self, path: &str) -> Result<(), String> {
                Err(format!("{path}: permission denied"))
            }
        }

        let (_directory, handle) = queue_handle().await;
        let batch_id = Uuid::from_u128(0xB6);
        let cancellation = handle.create_batch(batch_info(batch_id)).await.unwrap();
        let lister = FakeLister::new(expansion_tree());
        let sink = QueueSink::new(handle.clone(), batch_id);

        let outcome = run_expansion(
            &lister,
            &FailingDirCreator,
            &sink,
            &upload_plan(),
            &cancellation,
        )
        .await;

        assert_eq!(
            outcome,
            BatchExpansion::Interrupted {
                reason: "/dst: permission denied".into(),
            }
        );
        assert_eq!(lister.call_count(), 0, "nothing is walked without a home");
        assert!(handle.snapshot().jobs.is_empty());
    }

    #[tokio::test]
    async fn expansion_tolerates_trailing_slashes_on_discovered_paths() {
        let (_directory, handle) = queue_handle().await;
        let batch_id = Uuid::from_u128(0xB4);
        let cancellation = handle.create_batch(batch_info(batch_id)).await.unwrap();
        let mut tree = BTreeMap::new();
        tree.insert(
            "/src".to_string(),
            vec![WalkEntry::Dir {
                path: "/src/a/".into(),
            }],
        );
        tree.insert("/src/a/".to_string(), Vec::new());
        let lister = FakeLister::new(tree);
        let creator = RecordingDirCreator::default();
        let sink = QueueSink::new(handle.clone(), batch_id);

        let outcome = run_expansion(&lister, &creator, &sink, &upload_plan(), &cancellation).await;

        assert_eq!(outcome, BatchExpansion::Complete);
        assert_eq!(
            creator.created(),
            vec!["/dst".to_string(), "/dst/a".to_string()]
        );
    }
}
