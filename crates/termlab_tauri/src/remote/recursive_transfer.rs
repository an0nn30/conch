//! Recursive folder transfers: the listers, directory creators, and queue
//! sink that turn `transfer_enqueue_recursive` into ordinary member jobs.
//!
//! The backend owns expansion end to end — the frontend sends one command and
//! never walks a tree itself. Everything here reuses the pane's already-open
//! SSH session: expansion never authenticates, and it never consumes a
//! scheduler slot, because members are plain queue jobs and the walk itself is
//! I/O-light listing.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use termlab_remote::handler::TermLabSshHandler;
use termlab_remote::russh::client::Handle;
use termlab_remote::sftp::{DirEntryKind, list_dir_children};
use uuid::Uuid;

use super::RemoteState;
use super::transfer_queue::TransferQueueHandle;
use super::transfer_queue::batch::{BatchCancellation, BatchExpansion, BatchInfo};
use super::transfer_queue::expansion::{
    DirectoryCreator, DiscoveredFile, ExpansionPlan, ExpansionSink, ExpansionTotals, TreeLister,
    WalkEntry, run_expansion,
};
use super::transfer_queue::model::{NewTransferJob, TransferDirection};

type SshHandle = Arc<Handle<TermLabSshHandler>>;

// ---------------------------------------------------------------------------
// Listers
// ---------------------------------------------------------------------------

/// Walks the local filesystem for an upload. Mirrors `local_fs::list_dir`'s
/// ordering (directories first, then case-insensitive by name) but classifies
/// entries by `file_type`, which does not follow symlinks, and keeps hidden
/// files — a folder transfer copies the folder as it is on disk.
pub(super) struct LocalTreeLister;

#[async_trait]
impl TreeLister for LocalTreeLister {
    async fn list(&self, path: &str) -> Result<Vec<WalkEntry>, String> {
        let mut reader = tokio::fs::read_dir(path)
            .await
            .map_err(|error| error.to_string())?;
        let mut children = Vec::new();

        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|error| error.to_string())?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_path = join_path(path, &name);
            let file_type = match entry.file_type().await {
                Ok(file_type) => file_type,
                // A child that vanished mid-walk is not a reason to abandon
                // the whole tree.
                Err(error) => {
                    log::warn!("skipping {child_path} during expansion: {error}");
                    continue;
                }
            };
            let entry = if file_type.is_symlink() {
                WalkEntry::SkippedSymlink { path: child_path }
            } else if file_type.is_dir() {
                WalkEntry::Dir { path: child_path }
            } else {
                let size = entry.metadata().await.map(|meta| meta.len()).unwrap_or(0);
                WalkEntry::File {
                    path: child_path,
                    size,
                }
            };
            children.push((name, entry));
        }

        sort_children(&mut children);
        Ok(children.into_iter().map(|(_, entry)| entry).collect())
    }
}

/// Walks the remote tree for a download over the pane's existing session.
pub(super) struct SftpTreeLister {
    ssh: SshHandle,
}

#[async_trait]
impl TreeLister for SftpTreeLister {
    async fn list(&self, path: &str) -> Result<Vec<WalkEntry>, String> {
        let children = list_dir_children(&self.ssh, path)
            .await
            .map_err(|error| error.to_string())?;
        let mut children: Vec<_> = children
            .into_iter()
            .map(|child| {
                let child_path = join_path(path, &child.name);
                let entry = match child.kind {
                    DirEntryKind::Dir => WalkEntry::Dir { path: child_path },
                    DirEntryKind::File => WalkEntry::File {
                        path: child_path,
                        size: child.size,
                    },
                    // Sockets, fifos and devices have nothing to transfer, so
                    // they are recorded alongside symlinks as skipped.
                    DirEntryKind::Symlink | DirEntryKind::Other => {
                        WalkEntry::SkippedSymlink { path: child_path }
                    }
                };
                (child.name, entry)
            })
            .collect();

        sort_children(&mut children);
        Ok(children.into_iter().map(|(_, entry)| entry).collect())
    }
}

// ---------------------------------------------------------------------------
// Directory creators
// ---------------------------------------------------------------------------

/// Recreates directories locally for a download.
pub(super) struct LocalDirCreator;

#[async_trait]
impl DirectoryCreator for LocalDirCreator {
    async fn create_dir(&self, path: &str) -> Result<(), String> {
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|error| format!("{path}: {error}"))
    }
}

/// Recreates directories remotely for an upload. `mkdir` is single-level and
/// its failure statuses are not distinguishable through the error string, so
/// an existing directory is detected by stat rather than by error matching.
pub(super) struct SftpDirCreator {
    ssh: SshHandle,
}

#[async_trait]
impl DirectoryCreator for SftpDirCreator {
    async fn create_dir(&self, path: &str) -> Result<(), String> {
        match termlab_remote::sftp::stat(&self.ssh, path).await {
            Ok(entry) if entry.is_dir => return Ok(()),
            Ok(_) => return Err(format!("{path} exists and is not a directory")),
            Err(_) => {}
        }
        match termlab_remote::sftp::mkdir(&self.ssh, path).await {
            Ok(()) => Ok(()),
            // Lost a race with something else creating the same directory:
            // create-if-missing means that is still success.
            Err(error) => match termlab_remote::sftp::stat(&self.ssh, path).await {
                Ok(entry) if entry.is_dir => Ok(()),
                _ => Err(format!("{path}: {error}")),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Queue sink
// ---------------------------------------------------------------------------

/// Turns discovered files into ordinary member jobs and mirrors expansion
/// progress onto the batch record.
struct QueueExpansionSink {
    queue: TransferQueueHandle,
    remote: Arc<Mutex<RemoteState>>,
    batch_id: Uuid,
    direction: TransferDirection,
    caller_label: String,
    parent_label: Option<String>,
    pane_id: u32,
}

#[async_trait]
impl ExpansionSink for QueueExpansionSink {
    async fn enqueue_file(&self, file: DiscoveredFile) -> Result<(), String> {
        let request = self.build_request(&file)?;
        self.queue.enqueue(request).await.map(|_| ())
    }

    async fn record_batch(
        &self,
        expansion: BatchExpansion,
        totals: &ExpansionTotals,
    ) -> Result<(), String> {
        self.queue
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

impl QueueExpansionSink {
    fn build_request(&self, file: &DiscoveredFile) -> Result<NewTransferJob, String> {
        let (local_path, remote_path) = match self.direction {
            TransferDirection::Upload => (file.source_path.clone(), file.dest_path.clone()),
            TransferDirection::Download => (file.dest_path.clone(), file.source_path.clone()),
        };
        let state = self.remote.lock();
        super::transfer_commands::build_transfer_request(
            &*state,
            &self.caller_label,
            self.parent_label.as_deref(),
            self.pane_id,
            Uuid::new_v4(),
            self.direction.clone(),
            local_path,
            remote_path,
            None,
            None,
            Some(self.batch_id),
        )
    }
}

// ---------------------------------------------------------------------------
// Command entry point
// ---------------------------------------------------------------------------

/// Validate the source, create the batch, and spawn its expansion task.
/// Returns the batch id immediately — the walk continues in the background.
///
/// `dest_path` is the destination *container* — the directory the folder is
/// dropped into, which is what both callers named in the design supply (a
/// pane's current directory). The source folder is recreated inside it, so
/// `/local/assets` into `/srv/www` lands at `/srv/www/assets`, mirroring how
/// the single-file commands already receive a path built from the opposite
/// pane's current directory plus the entry name.
#[allow(clippy::too_many_arguments)]
pub(super) async fn start_recursive_transfer(
    queue: TransferQueueHandle,
    remote: Arc<Mutex<RemoteState>>,
    caller_label: String,
    parent_label: Option<String>,
    pane_id: u32,
    direction: TransferDirection,
    source_path: String,
    dest_path: String,
) -> Result<String, String> {
    if source_path.trim().is_empty() || dest_path.trim().is_empty() {
        return Err("recursive transfer needs a source and a destination".into());
    }

    // Resolving the session up front is what makes a disconnected pane fail
    // the command instead of failing silently inside the spawned task.
    let ssh = {
        let state = remote.lock();
        super::sftp_commands::get_ssh_handle(
            &state,
            parent_label.as_deref().unwrap_or(&caller_label),
            pane_id,
        )?
    };

    let (lister, creator): (Box<dyn TreeLister>, Box<dyn DirectoryCreator>) = match direction {
        TransferDirection::Upload => {
            validate_local_source_dir(&source_path)?;
            (
                Box::new(LocalTreeLister),
                Box::new(SftpDirCreator { ssh: ssh.clone() }),
            )
        }
        TransferDirection::Download => {
            let entry = termlab_remote::sftp::stat(&ssh, &source_path)
                .await
                .map_err(|error| format!("{source_path}: {error}"))?;
            if !entry.is_dir {
                return Err(format!("{source_path} is not a directory"));
            }
            (
                Box::new(SftpTreeLister { ssh: ssh.clone() }),
                Box::new(LocalDirCreator),
            )
        }
    };

    let batch_id = Uuid::new_v4();
    let name = batch_name(&source_path);
    let plan = ExpansionPlan {
        source_root: source_path.clone(),
        dest_root: join_path(&dest_path, &name),
        remote_dest: direction == TransferDirection::Upload,
    };
    let cancellation = queue
        .create_batch(BatchInfo {
            id: batch_id,
            name,
            direction: direction.clone(),
            expansion: BatchExpansion::Running,
            discovered_files: 0,
            discovered_bytes: 0,
            skipped: Vec::new(),
            created_at_ms: now_ms(),
        })
        .await?;

    let sink = QueueExpansionSink {
        queue: queue.clone(),
        remote,
        batch_id,
        direction,
        caller_label,
        parent_label,
        pane_id,
    };

    spawn_expansion(queue, batch_id, lister, creator, sink, plan, cancellation);
    Ok(batch_id.to_string())
}

/// Run the walk on its own task. The inner task is joined by an outer one so
/// that a panic anywhere in expansion still leaves the batch honestly marked
/// instead of stuck on "still counting" until the next restart.
fn spawn_expansion(
    queue: TransferQueueHandle,
    batch_id: Uuid,
    lister: Box<dyn TreeLister>,
    creator: Box<dyn DirectoryCreator>,
    sink: QueueExpansionSink,
    plan: ExpansionPlan,
    cancellation: BatchCancellation,
) {
    let walk = tauri::async_runtime::spawn(async move {
        run_expansion(
            lister.as_ref(),
            creator.as_ref(),
            &sink,
            &plan,
            &cancellation,
        )
        .await
    });

    tauri::async_runtime::spawn(async move {
        if let Err(error) = walk.await {
            log::error!("expansion task for batch {batch_id} failed: {error}");
            // Whatever the walk had already discovered is still true and its
            // members are still enqueued, so the totals are carried over
            // rather than reset to zero.
            let discovered = queue
                .snapshot()
                .batches
                .into_iter()
                .find(|aggregate| aggregate.info.id == batch_id)
                .map(|aggregate| aggregate.info);
            let _ = queue
                .update_batch(
                    batch_id,
                    BatchExpansion::Interrupted {
                        reason: "expansion task failed".into(),
                    },
                    discovered
                        .as_ref()
                        .map(|info| info.discovered_files)
                        .unwrap_or(0),
                    discovered
                        .as_ref()
                        .map(|info| info.discovered_bytes)
                        .unwrap_or(0),
                    discovered.map(|info| info.skipped).unwrap_or_default(),
                )
                .await;
        }
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The user-visible batch name: the source folder's own name.
pub(super) fn batch_name(source_path: &str) -> String {
    source_path
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(source_path)
        .to_string()
}

pub(super) fn validate_local_source_dir(source_path: &str) -> Result<(), String> {
    let metadata = std::fs::metadata(Path::new(source_path))
        .map_err(|error| format!("{source_path}: {error}"))?;
    if !metadata.is_dir() {
        return Err(format!("{source_path} is not a directory"));
    }
    Ok(())
}

/// Join a POSIX-style parent and child. Expansion keeps every path
/// '/'-separated because `map_destination` splits relatives on '/'.
fn join_path(parent: &str, name: &str) -> String {
    let parent = parent.trim_end_matches('/');
    if parent.is_empty() {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn sort_children(children: &mut [(String, WalkEntry)]) {
    children.sort_by(|(left_name, left), (right_name, right)| {
        let is_dir = |entry: &WalkEntry| matches!(entry, WalkEntry::Dir { .. });
        is_dir(right)
            .cmp(&is_dir(left))
            .then_with(|| left_name.to_lowercase().cmp(&right_name.to_lowercase()))
    });
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_name_is_the_source_folder_name() {
        assert_eq!(batch_name("/srv/releases/vendor-assets"), "vendor-assets");
        assert_eq!(batch_name("/srv/releases/vendor-assets/"), "vendor-assets");
        assert_eq!(batch_name("vendor-assets"), "vendor-assets");
        assert_eq!(batch_name("/"), "/");
    }

    #[test]
    fn join_path_keeps_one_separator_and_handles_the_root() {
        assert_eq!(join_path("/srv/tree", "a.txt"), "/srv/tree/a.txt");
        assert_eq!(join_path("/srv/tree/", "a.txt"), "/srv/tree/a.txt");
        assert_eq!(join_path("/", "a.txt"), "/a.txt");
    }

    #[test]
    fn local_source_validation_rejects_files_and_missing_paths() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("report.csv");
        std::fs::write(&file, b"data").unwrap();

        validate_local_source_dir(directory.path().to_str().unwrap()).unwrap();

        let file_error =
            validate_local_source_dir(file.to_str().unwrap()).expect_err("a file is not a folder");
        assert!(file_error.contains("is not a directory"), "{file_error}");

        let missing = directory.path().join("gone");
        let missing_error = validate_local_source_dir(missing.to_str().unwrap())
            .expect_err("a missing source must not create a batch");
        assert!(missing_error.contains("gone"), "{missing_error}");
    }

    #[tokio::test]
    async fn local_lister_orders_dirs_first_keeps_hidden_files_and_skips_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("zeta")).unwrap();
        std::fs::create_dir(root.join("alpha")).unwrap();
        std::fs::write(root.join(".hidden"), b"12345").unwrap();
        std::fs::write(root.join("beta.txt"), b"1234567890").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("alpha"), root.join("link")).unwrap();

        let entries = LocalTreeLister.list(root.to_str().unwrap()).await.unwrap();

        let names: Vec<_> = entries
            .iter()
            .map(|entry| {
                let path = match entry {
                    WalkEntry::Dir { path }
                    | WalkEntry::File { path, .. }
                    | WalkEntry::SkippedSymlink { path } => path,
                };
                path.rsplit('/').next().unwrap_or(path).to_string()
            })
            .collect();

        #[cfg(unix)]
        assert_eq!(names, vec!["alpha", "zeta", ".hidden", "beta.txt", "link"]);
        #[cfg(not(unix))]
        assert_eq!(names, vec!["alpha", "zeta", ".hidden", "beta.txt"]);

        assert!(
            entries
                .iter()
                .any(|entry| matches!(entry, WalkEntry::File { size: 5, .. })),
            "hidden files are included with their real size"
        );
        #[cfg(unix)]
        assert!(
            entries
                .iter()
                .any(|entry| matches!(entry, WalkEntry::SkippedSymlink { .. })),
            "a symlinked directory is recorded as skipped, never descended into"
        );
    }
}
